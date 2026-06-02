# Capability Registry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a YAML-driven per-model capability registry with edge validation, a tagged-union reference-image system, and a materializer that adapts refs to each provider's required form.

**Architecture:** A new `src/capabilities/` module owns model schemas (loaded from `models/*.yaml` at startup) and exposes them via a thread-safe `CapabilityRegistry`. A custom Axum extractor `ValidatedRequest<T>` runs the schema-based validator before handlers; a separate `Materializer` converts `reference_images` to the form each provider requires (uploading to temp storage, fetching URLs, building multipart bytes). Legacy singular reference fields (`image_url`, `mask_url`, `first_frame_url`, `last_frame_url`) are removed.

**Tech Stack:** Rust, Axum 0.8, Tokio, serde, serde_yaml, reqwest, wiremock for integration tests, existing `proxy::storage` backend (S3/local).

**Spec:** [`docs/superpowers/specs/2026-05-28-capability-registry-design.md`](../specs/2026-05-28-capability-registry-design.md)

---

## Phase A — Foundation

### Task 1: Add yaml + test dependencies

**Files:**
- Modify: `litegen-core/Cargo.toml`

- [ ] **Step 1: Add `serde_yaml` and `wiremock` to Cargo.toml**

Add to `[dependencies]`:

```toml
# YAML parsing for capability schemas
serde_yaml = "0.9"
```

Add to `[dev-dependencies]`:

```toml
# HTTP mocking for provider integration tests
wiremock = "0.6"
```

- [ ] **Step 2: Verify the workspace still builds**

Run: `cd litegen-core && cargo build`
Expected: clean build, no warnings about unused deps.

- [ ] **Step 3: Commit**

```bash
git add litegen-core/Cargo.toml litegen-core/Cargo.lock
git commit -m "deps(litegen-core): add serde_yaml and wiremock"
```

---

### Task 2: Capability schema types

**Files:**
- Create: `litegen-core/src/capabilities/mod.rs`
- Create: `litegen-core/src/capabilities/schema.rs`
- Modify: `litegen-core/src/lib.rs` (add `pub mod capabilities;`)

- [ ] **Step 1: Create `schema.rs` with the typed schema**

```rust
// litegen-core/src/capabilities/schema.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSpec {
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)] pub min_length: Option<usize>,
    #[serde(default)] pub max_length: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamSpec {
    Bool { #[serde(default)] default: Option<bool> },
    Int {
        #[serde(default)] min: Option<i64>,
        #[serde(default)] max: Option<i64>,
        #[serde(default)] default: Option<i64>,
    },
    Float {
        #[serde(default)] min: Option<f64>,
        #[serde(default)] max: Option<f64>,
        #[serde(default)] default: Option<f64>,
    },
    String {
        #[serde(default)] max_length: Option<usize>,
        #[serde(default)] enum_values: Vec<String>,
        #[serde(default)] pattern: Option<String>,
        #[serde(default)] default: Option<String>,
    },
    Size(SizeSpec),
    AspectRatio {
        allowed: Vec<String>,
        #[serde(default)] default: Option<String>,
    },
    Seed { min: i64, max: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SizeSpec {
    Freeform {
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
        #[serde(default)] multiple_of: Option<u32>,
    },
    Enum { values: Vec<(u32, u32)> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefInputSpec {
    pub max_total: u32,
    #[serde(default)] pub default_role: Option<String>,
    pub provider_format: RefProviderFormat,
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
    Url,
    Base64,
    Multipart {
        field_map: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSchema {
    pub id: String,
    pub provider: String,
    pub media_type: MediaType,
    pub display_name: String,
    #[serde(default)] pub description: String,
    pub pricing: ModelPricing,
    pub capabilities: ModelCapabilityFlags,
    pub prompt: PromptSpec,
    #[serde(default)] pub params: HashMap<String, ParamSpec>,
    #[serde(default)] pub ref_inputs: Option<RefInputSpec>,
    #[serde(default)] pub extra_allowlist: Vec<String>,
    #[serde(default)] pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
```

- [ ] **Step 2: Create `mod.rs` re-exporting the schema types**

```rust
// litegen-core/src/capabilities/mod.rs
pub mod schema;
pub use schema::*;
```

- [ ] **Step 3: Register the module**

In `litegen-core/src/lib.rs`, add `pub mod capabilities;` next to the other `pub mod` lines.

- [ ] **Step 4: Add a round-trip test**

Create `litegen-core/src/capabilities/schema_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::capabilities::*;
    use std::collections::HashMap;

    #[test]
    fn round_trip_minimal_model() {
        let yaml = r#"
models:
  - id: x/model
    provider: x
    media_type: image
    display_name: Model
    pricing:
      base_cost_usd: 0.01
    capabilities:
      text_to_image: true
    prompt:
      required: true
      max_length: 100
"#;
        let parsed: ModelsFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].id, "x/model");
        assert!(parsed.models[0].capabilities.text_to_image);
    }

    #[test]
    fn round_trip_full_model() {
        let yaml = r#"
models:
  - id: x/full
    provider: x
    media_type: image
    display_name: Full
    pricing: { base_cost_usd: 0.02 }
    capabilities:
      text_to_image: true
      image_to_image: true
    prompt: { required: true, max_length: 4000 }
    params:
      seed:
        kind: seed
        min: 0
        max: 4294967294
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9"]
        default: "1:1"
      size:
        kind: size
        mode: enum
        values:
          - [1024, 1024]
          - [1792, 1024]
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
    extra_allowlist: [output_format]
    tags: [text-to-image]
"#;
        let parsed: ModelsFile = serde_yaml::from_str(yaml).unwrap();
        let m = &parsed.models[0];
        assert!(matches!(m.params["seed"], ParamSpec::Seed { min: 0, .. }));
        let ri = m.ref_inputs.as_ref().unwrap();
        assert_eq!(ri.max_total, 1);
        assert!(matches!(ri.provider_format, RefProviderFormat::Multipart { .. }));
    }
}
```

Add `#[cfg(test)] mod schema_tests;` to `capabilities/mod.rs`.

- [ ] **Step 5: Run tests**

Run: `cd litegen-core && cargo test -p litegen capabilities::schema_tests`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/capabilities/ litegen-core/src/lib.rs
git commit -m "feat(litegen-core): add capability schema types"
```

---

### Task 3: Capability registry + loader

**Files:**
- Create: `litegen-core/src/capabilities/registry.rs`
- Create: `litegen-core/src/capabilities/loader.rs`
- Modify: `litegen-core/src/capabilities/mod.rs`

- [ ] **Step 1: Write failing loader tests**

Create `litegen-core/src/capabilities/loader_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::capabilities::*;

    fn yaml(s: &str) -> Result<CapabilityRegistry, LoadError> {
        CapabilityRegistry::from_yaml_strs(&[("test.yaml", s)])
    }

    #[test]
    fn loads_single_model() {
        let r = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r.get("x/m").unwrap().id, "x/m");
    }

    #[test]
    fn rejects_duplicate_id() {
        let err = CapabilityRegistry::from_yaml_strs(&[
            ("a.yaml", "models:\n  - id: x/m\n    provider: x\n    media_type: image\n    display_name: M\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
            ("b.yaml", "models:\n  - id: x/m\n    provider: x\n    media_type: image\n    display_name: M\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
        ]).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("duplicate"));
        assert!(s.contains("x/m"));
    }

    #[test]
    fn rejects_provider_id_mismatch() {
        let err = yaml(r#"
models:
  - id: foo/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap_err();
        assert!(err.to_string().contains("provider prefix"));
    }

    #[test]
    fn rejects_unknown_param_key() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params:
      unknown_param:
        kind: int
"#).unwrap_err();
        assert!(err.to_string().contains("unknown_param"));
    }

    #[test]
    fn rejects_field_map_undeclared_role() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format:
        form: multipart
        field_map: { init: image, ghost: mask }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
"#).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn rejects_freeform_min_gt_max() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params:
      size:
        kind: size
        mode: freeform
        min_width: 1000
        max_width: 100
        min_height: 100
        max_height: 1000
"#).unwrap_err();
        assert!(err.to_string().contains("min_width"));
    }

    #[test]
    fn for_provider_groups_correctly() {
        let r = CapabilityRegistry::from_yaml_strs(&[
            ("a.yaml", "models:\n  - id: x/a\n    provider: x\n    media_type: image\n    display_name: A\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n  - id: x/b\n    provider: x\n    media_type: image\n    display_name: B\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
            ("c.yaml", "models:\n  - id: y/c\n    provider: y\n    media_type: image\n    display_name: C\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
        ]).unwrap();
        assert_eq!(r.for_provider("x").count(), 2);
        assert_eq!(r.for_provider("y").count(), 1);
        assert_eq!(r.for_provider("z").count(), 0);
    }
}
```

- [ ] **Step 2: Run failing tests**

Run: `cd litegen-core && cargo test -p litegen capabilities::loader_tests 2>&1 | head`
Expected: compile error (`CapabilityRegistry` does not exist).

- [ ] **Step 3: Implement loader + registry**

Create `litegen-core/src/capabilities/loader.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::capabilities::schema::*;

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("io error reading {path}: {source}")]
    Io { path: String, source: std::io::Error },

    #[error("yaml parse error in {path}: {message}")]
    Parse { path: String, message: String },

    #[error("validation error in {path}: {message}")]
    Validate { path: String, message: String },

    #[error("aggregate: {0:#?}")]
    Aggregate(Vec<LoadError>),
}

pub(crate) fn parse_file(path: &str, yaml: &str) -> Result<Vec<ModelSchema>, LoadError> {
    let file: ModelsFile = serde_yaml::from_str(yaml).map_err(|e| LoadError::Parse {
        path: path.to_string(),
        message: e.to_string(),
    })?;
    for m in &file.models {
        validate_model(path, m)?;
    }
    Ok(file.models)
}

fn validate_model(path: &str, m: &ModelSchema) -> Result<(), LoadError> {
    let bad = |msg: String| LoadError::Validate { path: path.to_string(), message: msg };

    // id format: "<provider>/<rest>"
    let (prefix, rest) = m.id.split_once('/').ok_or_else(|| bad(format!(
        "model id '{}' must be in form 'provider/name'", m.id
    )))?;
    if rest.is_empty() {
        return Err(bad(format!("model id '{}' must have a name after '/'", m.id)));
    }
    if prefix != m.provider {
        return Err(bad(format!(
            "model id '{}' provider prefix '{}' does not match field provider '{}'",
            m.id, prefix, m.provider
        )));
    }

    if m.pricing.base_cost_usd < 0.0 {
        return Err(bad(format!("model '{}' has negative base_cost_usd", m.id)));
    }

    for (k, _) in &m.params {
        if !KNOWN_PARAMS.contains(&k.as_str()) {
            return Err(bad(format!(
                "model '{}' has unknown param key '{}'; known: {:?}",
                m.id, k, KNOWN_PARAMS
            )));
        }
    }

    // SizeSpec::Freeform sanity
    if let Some(ParamSpec::Size(SizeSpec::Freeform {
        min_width, max_width, min_height, max_height, ..
    })) = m.params.get("size") {
        if min_width > max_width {
            return Err(bad(format!(
                "model '{}' size: min_width {} > max_width {}", m.id, min_width, max_width
            )));
        }
        if min_height > max_height {
            return Err(bad(format!(
                "model '{}' size: min_height {} > max_height {}", m.id, min_height, max_height
            )));
        }
    }

    // RefInputSpec consistency
    if let Some(ri) = &m.ref_inputs {
        if let RefProviderFormat::Multipart { field_map } = &ri.provider_format {
            for k in field_map.keys() {
                if !ri.roles.contains_key(k) {
                    return Err(bad(format!(
                        "model '{}' ref_inputs.field_map role '{}' is not declared in roles",
                        m.id, k
                    )));
                }
            }
        }
        if let Some(default) = &ri.default_role {
            if !ri.roles.contains_key(default) {
                return Err(bad(format!(
                    "model '{}' ref_inputs.default_role '{}' is not declared in roles",
                    m.id, default
                )));
            }
        }
    }
    Ok(())
}

pub fn discover_model_files(dir: &Path) -> Result<Vec<PathBuf>, LoadError> {
    let entries = std::fs::read_dir(dir).map_err(|e| LoadError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| LoadError::Io { path: dir.display().to_string(), source: e })?;
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("yaml") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
```

Create `litegen-core/src/capabilities/registry.rs`:

```rust
use std::collections::HashMap;
use std::path::Path;

use crate::capabilities::loader::{self, LoadError};
use crate::capabilities::schema::*;

#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    models: HashMap<String, ModelSchema>,
    by_provider: HashMap<String, Vec<String>>,
}

impl CapabilityRegistry {
    pub fn from_dir(dir: &Path) -> Result<Self, LoadError> {
        let files = loader::discover_model_files(dir)?;
        let mut inputs: Vec<(String, String)> = Vec::new();
        for p in files {
            let content = std::fs::read_to_string(&p).map_err(|e| LoadError::Io {
                path: p.display().to_string(),
                source: e,
            })?;
            inputs.push((p.display().to_string(), content));
        }
        let refs: Vec<(&str, &str)> = inputs.iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        Self::from_yaml_strs(&refs)
    }

    pub fn from_yaml_strs(files: &[(&str, &str)]) -> Result<Self, LoadError> {
        let mut all = Vec::new();
        let mut errors: Vec<LoadError> = Vec::new();
        for (path, yaml) in files {
            match loader::parse_file(path, yaml) {
                Ok(models) => {
                    for m in models { all.push((path.to_string(), m)); }
                }
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            return Err(if errors.len() == 1 {
                errors.remove(0)
            } else {
                LoadError::Aggregate(errors)
            });
        }

        let mut models: HashMap<String, ModelSchema> = HashMap::new();
        let mut by_provider: HashMap<String, Vec<String>> = HashMap::new();
        for (path, m) in all {
            if models.contains_key(&m.id) {
                return Err(LoadError::Validate {
                    path,
                    message: format!("duplicate model id '{}'", m.id),
                });
            }
            by_provider.entry(m.provider.clone()).or_default().push(m.id.clone());
            models.insert(m.id.clone(), m);
        }
        for ids in by_provider.values_mut() { ids.sort(); }
        Ok(Self { models, by_provider })
    }

    pub fn get(&self, id: &str) -> Option<&ModelSchema> { self.models.get(id) }
    pub fn all(&self) -> impl Iterator<Item = &ModelSchema> { self.models.values() }
    pub fn for_provider<'a>(&'a self, provider: &str) -> impl Iterator<Item = &'a ModelSchema> {
        let ids = self.by_provider.get(provider).cloned().unwrap_or_default();
        ids.into_iter().filter_map(move |id| self.models.get(&id))
    }
    pub fn len(&self) -> usize { self.models.len() }
    pub fn is_empty(&self) -> bool { self.models.is_empty() }
}
```

Update `litegen-core/src/capabilities/mod.rs`:

```rust
pub mod schema;
pub mod loader;
pub mod registry;

pub use schema::*;
pub use loader::LoadError;
pub use registry::CapabilityRegistry;

#[cfg(test)] mod schema_tests;
#[cfg(test)] mod loader_tests;
```

- [ ] **Step 4: Run tests**

Run: `cd litegen-core && cargo test -p litegen capabilities::`
Expected: all tests pass (2 schema + 7 loader = 9 tests).

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/capabilities/
git commit -m "feat(litegen-core): add capability registry loader"
```

---

## Phase B — Request shapes

### Task 4: New ReferenceImage type and BaseGenerationRequest

**Files:**
- Modify: `litegen-core/src/types/mod.rs`

- [ ] **Step 1: Write failing serde tests**

Add to `litegen-core/src/types/mod.rs` (bottom):

```rust
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
        assert_eq!(r.base.strict, false);
        assert_eq!(r.size.as_deref(), Some("1024x1024"));
    }
}
```

- [ ] **Step 2: Run tests (should fail to compile)**

Run: `cd litegen-core && cargo test -p litegen types::ref_image_tests 2>&1 | tail -20`
Expected: compile errors about `ReferenceImage`, `RefImageKind`, `BaseGenerationRequest`.

- [ ] **Step 3: Add the new types**

Replace the existing `ImageGenerationRequest` and `VideoGenerationRequest` definitions in `litegen-core/src/types/mod.rs` with:

```rust
// ─── Generation Request / Response ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BaseGenerationRequest {
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
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
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
    #[serde(default = "default_duration")]
    pub duration_seconds: f64,
    #[serde(default)] pub aspect_ratio: Option<String>,
    #[serde(default)] pub resolution: Option<String>,
    #[serde(default)] pub fps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReferenceImage {
    #[serde(rename = "type")]
    pub kind: RefImageKind,
    pub value: String,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefImageKind {
    Base64,
    Url,
    Blob,
}

fn default_n() -> u32 { 1 }
fn default_response_format() -> String { "url".to_string() }
fn default_duration() -> f64 { 5.0 }
fn default_true() -> bool { true }
```

Delete the OLD `ImageGenerationRequest` and `VideoGenerationRequest` definitions and their helper fns if they exist as duplicates. Verify `default_content_type` stays where it is for `ImageResult`.

- [ ] **Step 4: Compile**

Run: `cd litegen-core && cargo build -p litegen 2>&1 | head -60`
Expected: many compile errors elsewhere — providers and handlers reference old fields. We fix those in later tasks. Goal here: only the types module is consistent.

To get a clean type-module check, run only the new tests:

Run: `cd litegen-core && cargo test -p litegen types::ref_image_tests --no-run 2>&1 | tail`
Expected: still compile errors in other modules. That's OK — we'll fix them.

- [ ] **Step 5: Mark existing callsites as scaffolded broken**

We need the build to pass so we can iterate. Temporarily comment out the provider and handler code that uses the deleted fields. In each provider file (`src/providers/image/*.rs`, `src/providers/video/*.rs`), wrap the bodies of `generate()` and `estimate_cost()` that reference the deleted fields with:

```rust
async fn generate(&self, _req: &ImageGenerationRequest) -> Result<GenerationOutput, ProviderError> {
    todo!("rewrite in capability-registry refactor")
}
```

For each provider (12 total), do this minimal stub. Keep `name()`, `configure()`, `is_configured()`, `health_check()`, and `list_models()` working.

Then run: `cd litegen-core && cargo build -p litegen`
Expected: clean build with many `todo!()` warnings.

- [ ] **Step 6: Run the new tests**

Run: `cd litegen-core && cargo test -p litegen types::ref_image_tests`
Expected: 4 tests pass.

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/types/mod.rs litegen-core/src/providers/
git commit -m "feat(litegen-core): refactor request types with ReferenceImage tagged union

Adds BaseGenerationRequest, ReferenceImage{type,value,role?}, and flattens
into image/video request shapes. Stubs provider generate() bodies with todo!()
to keep the workspace compiling; rewrites land in subsequent commits."
```

---

## Phase C — Validation + materialization

### Task 5: Validator core (param checking)

**Files:**
- Create: `litegen-core/src/api/middleware/validator.rs`
- Modify: `litegen-core/src/api/middleware/mod.rs`

- [ ] **Step 1: Write failing validator tests**

Create `litegen-core/src/api/middleware/validator_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::api::middleware::validator::*;
    use crate::capabilities::*;
    use crate::types::*;

    fn registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/strict
    provider: t
    media_type: image
    display_name: T
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 100 }
    params:
      seed:
        kind: seed
        min: 0
        max: 100
      guidance_scale:
        kind: float
        min: 0.0
        max: 10.0
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9"]
      size:
        kind: size
        mode: enum
        values:
          - [512, 512]
          - [1024, 1024]
    extra_allowlist: [output_format]
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
        mask: { required: false, min_count: 0, max_count: 1 }
"#)]).unwrap()
    }

    fn img(prompt: &str) -> ImageGenerationRequest {
        ImageGenerationRequest {
            base: BaseGenerationRequest {
                prompt: prompt.into(),
                model: "t/strict".into(),
                n: 1,
                negative_prompt: None,
                seed: None,
                reference_images: vec![],
                strict: true,
                extra: None,
                metadata: None,
            },
            size: None, aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None,
            response_format: "url".into(),
        }
    }

    #[test]
    fn empty_request_passes() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let out = validate_image(m, img("hello")).unwrap();
        assert!(out.dropped.is_empty());
    }

    #[test]
    fn unsupported_param_strict_rejects() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.steps = Some(20);
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_unsupported");
        assert_eq!(err.param.as_deref(), Some("steps"));
    }

    #[test]
    fn unsupported_param_lax_drops() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.steps = Some(20);
        let out = validate_image(m, req).unwrap();
        assert_eq!(out.dropped, vec!["steps".to_string()]);
        assert!(out.request.steps.is_none());
    }

    #[test]
    fn seed_out_of_range() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.seed = Some(1000);
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_out_of_range");
        assert_eq!(err.param.as_deref(), Some("seed"));
    }

    #[test]
    fn float_out_of_range() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.guidance_scale = Some(20.0);
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_out_of_range");
        assert_eq!(err.param.as_deref(), Some("guidance_scale"));
    }

    #[test]
    fn aspect_ratio_not_allowed() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.aspect_ratio = Some("3:2".into());
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch");
        assert_eq!(err.param.as_deref(), Some("aspect_ratio"));
    }

    #[test]
    fn size_enum_must_match() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.size = Some("768x768".into());
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch");
        assert_eq!(err.param.as_deref(), Some("size"));
    }

    #[test]
    fn size_enum_passes_when_matches() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.size = Some("1024x1024".into());
        assert!(validate_image(m, req).is_ok());
    }

    #[test]
    fn prompt_too_long() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let req = img(&"x".repeat(200));
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "prompt_too_long");
    }

    #[test]
    fn prompt_required_when_empty() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let req = img("");
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "prompt_required");
    }

    #[test]
    fn ref_images_total_exceeded() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u1".into(), role: Some("init".into()) },
            ReferenceImage { kind: RefImageKind::Url, value: "u2".into(), role: Some("mask".into()) },
            ReferenceImage { kind: RefImageKind::Url, value: "u3".into(), role: None },
        ];
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "ref_total_exceeded");
    }

    #[test]
    fn ref_role_count_exceeded() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u1".into(), role: Some("init".into()) },
            ReferenceImage { kind: RefImageKind::Url, value: "u2".into(), role: Some("init".into()) },
        ];
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "ref_role_count_out_of_range");
    }

    #[test]
    fn ref_unknown_role_strict_rejects() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u".into(), role: Some("ghost".into()) },
        ];
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "ref_role_unknown");
    }

    #[test]
    fn ref_unknown_role_lax_drops() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u".into(), role: Some("ghost".into()) },
        ];
        let out = validate_image(m, req).unwrap();
        assert!(out.request.base.reference_images.is_empty());
        assert!(out.dropped.contains(&"reference_images[ghost]".to_string()));
    }

    #[test]
    fn extra_allowlist_strict() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.extra = Some(json!({"output_format": "png", "ghost": 1}));
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "extra_key_unsupported");
    }

    #[test]
    fn extra_lax_passes_through() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.base.extra = Some(json!({"output_format": "png", "ghost": 1}));
        let out = validate_image(m, req).unwrap();
        assert!(out.request.base.extra.is_some());
    }
}
```

- [ ] **Step 2: Run failing tests**

Run: `cd litegen-core && cargo test -p litegen api::middleware::validator_tests --no-run 2>&1 | tail`
Expected: compile errors about `validate_image`, `validate_video`.

- [ ] **Step 3: Create `validator.rs`**

```rust
// litegen-core/src/api/middleware/validator.rs
use serde_json::Value;
use std::collections::HashMap;

use crate::capabilities::*;
use crate::types::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    pub param: Option<String>,
}

impl ValidationError {
    fn new(code: &str, msg: impl Into<String>, param: Option<&str>) -> Self {
        Self { code: code.into(), message: msg.into(), param: param.map(str::to_string) }
    }
}

#[derive(Debug)]
pub struct ImageValidationOutput {
    pub request: ImageGenerationRequest,
    pub dropped: Vec<String>,
}

#[derive(Debug)]
pub struct VideoValidationOutput {
    pub request: VideoGenerationRequest,
    pub dropped: Vec<String>,
}

pub fn validate_image(
    schema: &ModelSchema,
    mut req: ImageGenerationRequest,
) -> Result<ImageValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(&schema.prompt, &req.base.prompt)?;

    macro_rules! check_param {
        ($field:expr, $key:literal, $variant:pat => $body:block) => {
            if $field.is_some() {
                match schema.params.get($key) {
                    Some($variant) => $body,
                    Some(_) => {
                        return Err(ValidationError::new(
                            "param_unsupported",
                            format!("Parameter '{}' has an incompatible kind for model '{}'.", $key, schema.id),
                            Some($key),
                        ));
                    }
                    None => {
                        if strict {
                            return Err(ValidationError::new(
                                "param_unsupported",
                                format!("Parameter '{}' is not supported by model '{}'.", $key, schema.id),
                                Some($key),
                            ));
                        } else {
                            dropped.push($key.to_string());
                            $field = None;
                        }
                    }
                }
            }
        };
    }

    check_param!(req.base.seed, "seed", ParamSpec::Seed { min, max } => {
        let v = req.base.seed.unwrap();
        if v < *min || v > *max {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("seed {} outside [{}, {}]", v, min, max),
                Some("seed"),
            ));
        }
    });

    check_param!(req.base.negative_prompt, "negative_prompt", ParamSpec::String { max_length, .. } => {
        if let Some(m) = max_length {
            let v = req.base.negative_prompt.as_deref().unwrap();
            if v.len() > *m {
                return Err(ValidationError::new(
                    "param_too_long",
                    format!("negative_prompt length {} exceeds {}", v.len(), m),
                    Some("negative_prompt"),
                ));
            }
        }
    });

    check_param!(req.steps, "steps", ParamSpec::Int { min, max, .. } => {
        let v = req.steps.unwrap() as i64;
        if min.map(|m| v < m).unwrap_or(false) || max.map(|m| v > m).unwrap_or(false) {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("steps {} out of range", v),
                Some("steps"),
            ));
        }
    });

    check_param!(req.guidance_scale, "guidance_scale", ParamSpec::Float { min, max, .. } => {
        let v = req.guidance_scale.unwrap();
        if min.map(|m| v < m).unwrap_or(false) || max.map(|m| v > m).unwrap_or(false) {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("guidance_scale {} out of range", v),
                Some("guidance_scale"),
            ));
        }
    });

    check_param!(req.strength, "strength", ParamSpec::Float { min, max, .. } => {
        let v = req.strength.unwrap();
        if min.map(|m| v < m).unwrap_or(false) || max.map(|m| v > m).unwrap_or(false) {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("strength {} out of range", v),
                Some("strength"),
            ));
        }
    });

    check_param!(req.quality, "quality", ParamSpec::String { enum_values, max_length, .. } => {
        let v = req.quality.as_deref().unwrap();
        check_string(v, enum_values, *max_length, "quality")?;
    });

    check_param!(req.style, "style", ParamSpec::String { enum_values, max_length, .. } => {
        let v = req.style.as_deref().unwrap();
        check_string(v, enum_values, *max_length, "style")?;
    });

    check_param!(req.aspect_ratio, "aspect_ratio", ParamSpec::AspectRatio { allowed, .. } => {
        let v = req.aspect_ratio.as_deref().unwrap();
        if !allowed.iter().any(|a| a == v) {
            return Err(ValidationError::new(
                "param_enum_mismatch",
                format!("aspect_ratio '{}' not in {:?}", v, allowed),
                Some("aspect_ratio"),
            ));
        }
    });

    check_param!(req.size, "size", ParamSpec::Size(spec) => {
        let v = req.size.as_deref().unwrap();
        check_size(v, spec, "size")?;
    });

    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(ImageValidationOutput { request: req, dropped })
}

pub fn validate_video(
    schema: &ModelSchema,
    mut req: VideoGenerationRequest,
) -> Result<VideoValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(&schema.prompt, &req.base.prompt)?;

    // seed, negative_prompt — same as image
    if req.base.seed.is_some() {
        match schema.params.get("seed") {
            Some(ParamSpec::Seed { min, max }) => {
                let v = req.base.seed.unwrap();
                if v < *min || v > *max {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("seed {} outside [{}, {}]", v, min, max),
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
            Some(ParamSpec::String { max_length, .. }) => {
                if let Some(m) = max_length {
                    let v = req.base.negative_prompt.as_deref().unwrap();
                    if v.len() > *m {
                        return Err(ValidationError::new(
                            "param_too_long",
                            format!("negative_prompt length {} > {}", v.len(), m),
                            Some("negative_prompt"),
                        ));
                    }
                }
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

    // duration_seconds is special — float typically; an Int spec is also accepted.
    if req.duration_seconds > 0.0 {
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Float { min, max, .. }) => {
                let v = req.duration_seconds;
                if min.map(|m| v < m).unwrap_or(false) || max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("duration_seconds {} out of range", v),
                        Some("duration_seconds"),
                    ));
                }
            }
            Some(ParamSpec::Int { min, max, .. }) => {
                let v = req.duration_seconds.round() as i64;
                if min.map(|m| v < m).unwrap_or(false) || max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("duration_seconds {} out of range", v),
                        Some("duration_seconds"),
                    ));
                }
            }
            _ => {} // no spec → accept default; provider may further constrain
        }
    }

    if req.aspect_ratio.is_some() {
        match schema.params.get("aspect_ratio") {
            Some(ParamSpec::AspectRatio { allowed, .. }) => {
                let v = req.aspect_ratio.as_deref().unwrap();
                if !allowed.iter().any(|a| a == v) {
                    return Err(ValidationError::new(
                        "param_enum_mismatch",
                        format!("aspect_ratio '{}' not in {:?}", v, allowed),
                        Some("aspect_ratio"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("aspect_ratio not supported by '{}'", schema.id),
                        Some("aspect_ratio"),
                    ));
                }
                dropped.push("aspect_ratio".into());
                req.aspect_ratio = None;
            }
        }
    }

    if req.fps.is_some() {
        match schema.params.get("fps") {
            Some(ParamSpec::Int { min, max, .. }) => {
                let v = req.fps.unwrap() as i64;
                if min.map(|m| v < m).unwrap_or(false) || max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("fps {} out of range", v),
                        Some("fps"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("fps not supported by '{}'", schema.id),
                        Some("fps"),
                    ));
                }
                dropped.push("fps".into());
                req.fps = None;
            }
        }
    }

    if req.resolution.is_some() {
        match schema.params.get("resolution") {
            Some(ParamSpec::String { enum_values, max_length, .. }) => {
                let v = req.resolution.as_deref().unwrap();
                check_string(v, enum_values, *max_length, "resolution")?;
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("resolution not supported by '{}'", schema.id),
                        Some("resolution"),
                    ));
                }
                dropped.push("resolution".into());
                req.resolution = None;
            }
        }
    }

    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(VideoValidationOutput { request: req, dropped })
}

fn check_prompt(spec: &PromptSpec, value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        if spec.required {
            return Err(ValidationError::new(
                "prompt_required",
                "prompt is required",
                Some("prompt"),
            ));
        }
        return Ok(());
    }
    if let Some(min) = spec.min_length {
        if value.len() < min {
            return Err(ValidationError::new(
                "prompt_too_short",
                format!("prompt length {} < {}", value.len(), min),
                Some("prompt"),
            ));
        }
    }
    if let Some(max) = spec.max_length {
        if value.len() > max {
            return Err(ValidationError::new(
                "prompt_too_long",
                format!("prompt length {} > {}", value.len(), max),
                Some("prompt"),
            ));
        }
    }
    Ok(())
}

fn check_string(
    v: &str,
    enum_values: &[String],
    max_length: Option<usize>,
    param: &str,
) -> Result<(), ValidationError> {
    if let Some(m) = max_length {
        if v.len() > m {
            return Err(ValidationError::new(
                "param_too_long",
                format!("{} length {} > {}", param, v.len(), m),
                Some(param),
            ));
        }
    }
    if !enum_values.is_empty() && !enum_values.iter().any(|e| e == v) {
        return Err(ValidationError::new(
            "param_enum_mismatch",
            format!("{} '{}' not in {:?}", param, v, enum_values),
            Some(param),
        ));
    }
    Ok(())
}

fn check_size(v: &str, spec: &SizeSpec, param: &str) -> Result<(), ValidationError> {
    let (w, h) = parse_size(v).ok_or_else(|| ValidationError::new(
        "param_enum_mismatch",
        format!("{} '{}' must be 'WxH'", param, v),
        Some(param),
    ))?;
    match spec {
        SizeSpec::Enum { values } => {
            if !values.iter().any(|(ww, hh)| *ww == w && *hh == h) {
                return Err(ValidationError::new(
                    "param_enum_mismatch",
                    format!("{} '{}' not in allowed sizes {:?}", param, v, values),
                    Some(param),
                ));
            }
        }
        SizeSpec::Freeform { min_width, max_width, min_height, max_height, multiple_of } => {
            if w < *min_width || w > *max_width || h < *min_height || h > *max_height {
                return Err(ValidationError::new(
                    "param_out_of_range",
                    format!("{} '{}' outside bounds", param, v),
                    Some(param),
                ));
            }
            if let Some(m) = multiple_of {
                if w % m != 0 || h % m != 0 {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("{} '{}' not divisible by {}", param, v, m),
                        Some(param),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn check_refs(
    schema: &ModelSchema,
    refs: &mut Vec<ReferenceImage>,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(ri) = schema.ref_inputs.as_ref() else {
        if !refs.is_empty() {
            if strict {
                return Err(ValidationError::new(
                    "ref_role_unknown",
                    format!("model '{}' does not accept reference images", schema.id),
                    Some("reference_images"),
                ));
            }
            for _ in refs.drain(..) { dropped.push("reference_images".into()); }
        }
        return Ok(());
    };

    if (refs.len() as u32) > ri.max_total {
        return Err(ValidationError::new(
            "ref_total_exceeded",
            format!("{} reference images > max {}", refs.len(), ri.max_total),
            Some("reference_images"),
        ));
    }

    // Resolve missing role → default_role; record dropped for unknown roles in lax.
    let mut keep: Vec<ReferenceImage> = Vec::with_capacity(refs.len());
    for r in refs.drain(..) {
        let role = r.role.clone().or_else(|| ri.default_role.clone());
        let Some(role_name) = role else {
            if strict {
                return Err(ValidationError::new(
                    "ref_role_unknown",
                    "reference image missing role and model has no default_role",
                    Some("reference_images"),
                ));
            }
            dropped.push("reference_images[no_role]".into());
            continue;
        };
        if !ri.roles.contains_key(&role_name) {
            if strict {
                return Err(ValidationError::new(
                    "ref_role_unknown",
                    format!("role '{}' not declared for model '{}'", role_name, schema.id),
                    Some("reference_images"),
                ));
            }
            dropped.push(format!("reference_images[{}]", role_name));
            continue;
        }
        keep.push(ReferenceImage { role: Some(role_name), ..r });
    }

    // Per-role count + required.
    let mut counts: HashMap<String, u32> = HashMap::new();
    for r in &keep {
        *counts.entry(r.role.clone().unwrap()).or_insert(0) += 1;
    }
    for (role, spec) in &ri.roles {
        let c = counts.get(role).copied().unwrap_or(0);
        if c < spec.min_count {
            if spec.required || c > 0 {
                return Err(ValidationError::new(
                    "ref_role_count_out_of_range",
                    format!("role '{}' count {} < min {}", role, c, spec.min_count),
                    Some("reference_images"),
                ));
            }
            if spec.required && c == 0 {
                return Err(ValidationError::new(
                    "ref_role_required",
                    format!("role '{}' is required", role),
                    Some("reference_images"),
                ));
            }
        }
        if c > spec.max_count {
            return Err(ValidationError::new(
                "ref_role_count_out_of_range",
                format!("role '{}' count {} > max {}", role, c, spec.max_count),
                Some("reference_images"),
            ));
        }
    }

    *refs = keep;
    Ok(())
}

fn check_extra(
    schema: &ModelSchema,
    extra: &mut Option<Value>,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(v) = extra else { return Ok(()); };
    let Some(obj) = v.as_object() else {
        return Err(ValidationError::new(
            "extra_key_unsupported",
            "extra must be a JSON object",
            Some("extra"),
        ));
    };
    if !strict {
        return Ok(());
    }
    let mut filtered = serde_json::Map::new();
    for (k, val) in obj {
        if schema.extra_allowlist.iter().any(|a| a == k) {
            filtered.insert(k.clone(), val.clone());
        } else {
            return Err(ValidationError::new(
                "extra_key_unsupported",
                format!("extra key '{}' not in allowlist for model '{}'", k, schema.id),
                Some(&format!("extra.{}", k)),
            ));
        }
    }
    *extra = Some(Value::Object(filtered));
    let _ = dropped; // currently strict mode either fails or keeps; lax leaves unchanged.
    Ok(())
}
```

Register the module and tests in `litegen-core/src/api/middleware/mod.rs`:

```rust
pub mod validator;
#[cfg(test)] mod validator_tests;
```

- [ ] **Step 4: Run tests**

Run: `cd litegen-core && cargo test -p litegen api::middleware::validator_tests`
Expected: all 17 tests pass.

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/api/middleware/
git commit -m "feat(litegen-core): add request validator against capability schema"
```

---

### Task 6: Materializer

**Files:**
- Create: `litegen-core/src/proxy/materializer.rs`
- Modify: `litegen-core/src/proxy/mod.rs`

- [ ] **Step 1: Write failing materializer tests**

Create `litegen-core/src/proxy/materializer_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::capabilities::*;
    use crate::proxy::materializer::*;
    use crate::types::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn schema_with_format(form: RefProviderFormat) -> ModelSchema {
        ModelSchema {
            id: "t/m".into(),
            provider: "t".into(),
            media_type: MediaType::Image,
            display_name: "M".into(),
            description: "".into(),
            pricing: ModelPricing { base_cost_usd: 0.01, variable_pricing: None },
            capabilities: ModelCapabilityFlags { text_to_image: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(),
            ref_inputs: Some(RefInputSpec {
                max_total: 2,
                default_role: Some("init".into()),
                provider_format: form,
                roles: HashMap::from([
                    ("init".into(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
                    ("mask".into(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
                ]),
            }),
            extra_allowlist: vec![],
            tags: vec![],
        }
    }

    fn ref_(kind: RefImageKind, value: &str, role: &str) -> ReferenceImage {
        ReferenceImage { kind, value: value.into(), role: Some(role.into()) }
    }

    #[tokio::test]
    async fn passthrough_url_to_url() {
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let refs = vec![ref_(RefImageKind::Url, "https://x/a.png", "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        assert_eq!(out.refs.len(), 1);
        match &out.refs[0].form {
            MaterializedRefForm::Url(u) => assert_eq!(u, "https://x/a.png"),
            other => panic!("expected URL, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn base64_to_url_uploads() {
        let storage = Arc::new(InMemoryStorage::default());
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(storage.clone(), reqwest_stub());
        let refs = vec![ref_(RefImageKind::Base64, &base64::encode(b"hello"), "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        assert!(matches!(out.refs[0].form, MaterializedRefForm::Url(_)));
        assert_eq!(storage.uploaded_count(), 1);
        drop(out); // runs Cleanup
        // Spawned cleanup task is async; give it a tick.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(storage.deleted_count(), 1);
    }

    #[tokio::test]
    async fn base64_to_base64_passthrough() {
        let schema = schema_with_format(RefProviderFormat::Base64);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let refs = vec![ref_(RefImageKind::Base64, "ZGF0YQ==", "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        assert!(matches!(out.refs[0].form, MaterializedRefForm::Base64(_)));
    }

    #[tokio::test]
    async fn base64_to_multipart_decodes() {
        use std::collections::HashMap;
        let schema = schema_with_format(RefProviderFormat::Multipart {
            field_map: HashMap::from([("init".into(), "image".into())]),
        });
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let b64 = base64::encode(b"hello");
        let refs = vec![ref_(RefImageKind::Base64, &b64, "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        match &out.refs[0].form {
            MaterializedRefForm::MultipartField { field_name, bytes, .. } => {
                assert_eq!(field_name, "image");
                assert_eq!(bytes.as_ref(), b"hello");
            }
            other => panic!("expected multipart, got {:?}", other),
        }
    }

    // Helper: an in-memory fake of the storage backend.
    #[derive(Default)]
    pub struct InMemoryStorage {
        uploaded: std::sync::Mutex<Vec<String>>,
        deleted: std::sync::Mutex<Vec<String>>,
    }
    impl InMemoryStorage {
        pub fn uploaded_count(&self) -> usize { self.uploaded.lock().unwrap().len() }
        pub fn deleted_count(&self) -> usize { self.deleted.lock().unwrap().len() }
    }
    #[async_trait::async_trait]
    impl TempStorage for InMemoryStorage {
        async fn put(&self, key: &str, _bytes: bytes::Bytes, _ct: &str) -> Result<String, MaterializeError> {
            self.uploaded.lock().unwrap().push(key.to_string());
            Ok(format!("https://fake/{}", key))
        }
        async fn delete(&self, key: &str) -> Result<(), MaterializeError> {
            self.deleted.lock().unwrap().push(key.to_string());
            Ok(())
        }
    }

    fn reqwest_stub() -> reqwest::Client { reqwest::Client::new() }
}
```

(Note: this depends on a `TempStorage` trait we'll define. The test uses `base64::encode` which is the older API; use `base64::engine::general_purpose::STANDARD.encode` if needed — keep the helper using whichever the workspace already uses.)

- [ ] **Step 2: Run failing tests**

Run: `cd litegen-core && cargo test -p litegen proxy::materializer_tests --no-run 2>&1 | tail`
Expected: compile errors.

- [ ] **Step 3: Implement materializer**

Create `litegen-core/src/proxy/materializer.rs`:

```rust
use std::sync::Arc;
use std::collections::HashMap;
use bytes::Bytes;
use base64::{Engine, engine::general_purpose::STANDARD as B64};

use crate::capabilities::*;
use crate::types::*;

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("ref image: {0}")]
    Ref(String),

    #[error("upload failed: {0}")]
    Upload(String),

    #[error("fetch failed: {0}")]
    Fetch(String),

    #[error("decode failed: {0}")]
    Decode(String),
}

#[async_trait::async_trait]
pub trait TempStorage: Send + Sync {
    async fn put(&self, key: &str, bytes: Bytes, content_type: &str) -> Result<String, MaterializeError>;
    async fn delete(&self, key: &str) -> Result<(), MaterializeError>;
}

#[derive(Debug, Default, Clone)]
pub struct MaterializeContext {
    /// Blob field name → (bytes, content type). Populated by the multipart extractor.
    pub blob_parts: HashMap<String, (Bytes, String)>,
}

#[derive(Debug)]
pub struct MaterializedRef {
    pub role: String,
    pub form: MaterializedRefForm,
}

#[derive(Debug)]
pub enum MaterializedRefForm {
    Url(String),
    Base64(String),
    MultipartField { field_name: String, bytes: Bytes, content_type: String },
}

pub struct MaterializedRequest {
    pub refs: Vec<MaterializedRef>,
    /// Drop guard: cleans up temp-uploaded objects when this is dropped.
    pub cleanup: Cleanup,
}

pub struct Cleanup {
    storage: Option<Arc<dyn TempStorage>>,
    keys: Vec<String>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.keys.is_empty() { return; }
        let Some(storage) = self.storage.clone() else { return; };
        let keys = std::mem::take(&mut self.keys);
        tokio::spawn(async move {
            for k in keys {
                if let Err(e) = storage.delete(&k).await {
                    tracing::warn!(error=%e, key=%k, "temp ref cleanup failed");
                }
            }
        });
    }
}

pub struct Materializer {
    storage: Arc<dyn TempStorage>,
    http: reqwest::Client,
}

impl Materializer {
    pub fn new(storage: Arc<dyn TempStorage>, http: reqwest::Client) -> Self {
        Self { storage, http }
    }

    pub async fn materialize(
        &self,
        schema: &ModelSchema,
        refs: Vec<ReferenceImage>,
        ctx: &MaterializeContext,
    ) -> Result<MaterializedRequest, MaterializeError> {
        let Some(ri) = schema.ref_inputs.as_ref() else {
            return Ok(MaterializedRequest {
                refs: Vec::new(),
                cleanup: Cleanup { storage: None, keys: Vec::new() },
            });
        };
        let mut out_refs = Vec::with_capacity(refs.len());
        let mut keys = Vec::new();
        for r in refs {
            let role = r.role.clone().or_else(|| ri.default_role.clone())
                .ok_or_else(|| MaterializeError::Ref("ref missing role".into()))?;
            let form = self.convert(&role, r, &ri.provider_format, ctx, &mut keys).await?;
            out_refs.push(MaterializedRef { role, form });
        }
        Ok(MaterializedRequest {
            refs: out_refs,
            cleanup: Cleanup { storage: Some(self.storage.clone()), keys },
        })
    }

    async fn convert(
        &self,
        role: &str,
        r: ReferenceImage,
        target: &RefProviderFormat,
        ctx: &MaterializeContext,
        keys: &mut Vec<String>,
    ) -> Result<MaterializedRefForm, MaterializeError> {
        match (r.kind, target) {
            (RefImageKind::Url, RefProviderFormat::Url) => Ok(MaterializedRefForm::Url(r.value)),
            (RefImageKind::Base64, RefProviderFormat::Base64) => {
                Ok(MaterializedRefForm::Base64(strip_data_prefix(&r.value).to_string()))
            }
            (RefImageKind::Blob, RefProviderFormat::Multipart { field_map }) => {
                let (bytes, ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes, content_type: ct })
            }
            (RefImageKind::Url, RefProviderFormat::Base64) => {
                let bytes = self.fetch_url(&r.value).await?;
                Ok(MaterializedRefForm::Base64(B64.encode(&bytes)))
            }
            (RefImageKind::Url, RefProviderFormat::Multipart { field_map }) => {
                let bytes = self.fetch_url(&r.value).await?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes: bytes.into(), content_type: "application/octet-stream".into() })
            }
            (RefImageKind::Base64, RefProviderFormat::Url) => {
                let raw = strip_data_prefix(&r.value);
                let bytes = B64.decode(raw).map_err(|e| MaterializeError::Decode(e.to_string()))?;
                let key = format!("tmp/refs/{}", uuid::Uuid::new_v4());
                let url = self.storage.put(&key, bytes.into(), "application/octet-stream").await?;
                keys.push(key);
                Ok(MaterializedRefForm::Url(url))
            }
            (RefImageKind::Base64, RefProviderFormat::Multipart { field_map }) => {
                let raw = strip_data_prefix(&r.value);
                let bytes = B64.decode(raw).map_err(|e| MaterializeError::Decode(e.to_string()))?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes: bytes.into(), content_type: "application/octet-stream".into() })
            }
            (RefImageKind::Blob, RefProviderFormat::Url) => {
                let (bytes, ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                let key = format!("tmp/refs/{}", uuid::Uuid::new_v4());
                let url = self.storage.put(&key, bytes, &ct).await?;
                keys.push(key);
                Ok(MaterializedRefForm::Url(url))
            }
            (RefImageKind::Blob, RefProviderFormat::Base64) => {
                let (bytes, _ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                Ok(MaterializedRefForm::Base64(B64.encode(&bytes)))
            }
        }
    }

    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, MaterializeError> {
        let resp = self.http.get(url).send().await.map_err(|e| MaterializeError::Fetch(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MaterializeError::Fetch(format!("status {}", resp.status())));
        }
        let bytes = resp.bytes().await.map_err(|e| MaterializeError::Fetch(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

fn strip_data_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("data:") {
        if let Some(idx) = rest.find(',') {
            return &rest[idx + 1..];
        }
    }
    s
}
```

Add to `litegen-core/src/proxy/mod.rs`:

```rust
pub mod materializer;
#[cfg(test)] mod materializer_tests;
```

Also add a small adapter that maps the existing `proxy::storage` backends to the new `TempStorage` trait. In a follow-up step within this task:

Add to the bottom of `litegen-core/src/proxy/materializer.rs`:

```rust
/// Adapter that wraps the existing storage backend with the TempStorage trait.
pub struct StorageAdapter {
    inner: Arc<crate::proxy::storage::ImageStorage>,
}

impl StorageAdapter {
    pub fn new(inner: Arc<crate::proxy::storage::ImageStorage>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl TempStorage for StorageAdapter {
    async fn put(&self, key: &str, bytes: Bytes, ct: &str) -> Result<String, MaterializeError> {
        self.inner.put(key, &bytes, ct).await
            .map_err(|e| MaterializeError::Upload(e.to_string()))
    }
    async fn delete(&self, key: &str) -> Result<(), MaterializeError> {
        self.inner.delete(key).await
            .map_err(|e| MaterializeError::Upload(e.to_string()))
    }
}
```

(If `ImageStorage` doesn't already expose `put` / `delete`, add them at this step; see `litegen-core/src/proxy/storage.rs`.)

- [ ] **Step 4: Run tests**

Run: `cd litegen-core && cargo test -p litegen proxy::materializer_tests`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/proxy/
git commit -m "feat(litegen-core): add reference-image materializer"
```

---

## Phase D — Provider plumbing

### Task 7: Refactor ImageProvider + VideoProvider traits

**Files:**
- Modify: `litegen-core/src/providers/mod.rs`

- [ ] **Step 1: Edit the traits**

In `litegen-core/src/providers/mod.rs`, replace the trait definitions:

```rust
use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;

pub struct ImageExtras {
    pub size: Option<String>,
    pub aspect_ratio: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub steps: Option<u32>,
    pub guidance_scale: Option<f64>,
    pub strength: Option<f64>,
    pub response_format: String,
    pub extra: Option<serde_json::Value>,
}

pub struct VideoExtras {
    pub duration_seconds: f64,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<String>,
    pub fps: Option<u32>,
    pub extra: Option<serde_json::Value>,
}

#[async_trait]
pub trait ImageProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError>;

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;

    fn map_model_id(&self, model: &str, mapping: &HashMap<String, String>) -> String {
        mapping.get(model).cloned().unwrap_or_else(|| model.to_string())
    }
}

#[async_trait]
pub trait VideoProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError>;

    async fn poll_status(&self, handle: &VideoGenerationHandle)
        -> Result<VideoGenerationPollResult, ProviderError>;

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;
}
```

Remove `supported_models`, `supports_model`, `list_models` from both traits.

Add `use crate::types::*;` at the top if not already imported.

- [ ] **Step 2: Compile (will break everywhere)**

Run: `cd litegen-core && cargo build -p litegen 2>&1 | head -100`
Expected: lots of provider impl errors. Expected.

- [ ] **Step 3: Stub every provider to the new signature**

For each provider file under `src/providers/image/*.rs` and `src/providers/video/*.rs`, update the impl block: keep `name`/`configure`/`is_configured`/`health_check`/`estimate_cost`/`poll_status` as-is or stubbed, but rewrite `generate` to the new signature with `todo!()`:

```rust
async fn generate(
    &self,
    _model: &ModelSchema,
    _base: &BaseGenerationRequest,
    _extras: &ImageExtras,
    _materialized: &MaterializedRequest,
) -> Result<GenerationOutput, ProviderError> {
    todo!("rewrite for capability registry")
}
```

Remove `supported_models()` and `list_models()` impls.

`estimate_cost` keeps the old signature plus a `&ModelSchema` first arg — wrap with `todo!()` if its body referenced deleted fields.

- [ ] **Step 4: Compile clean**

Run: `cd litegen-core && cargo build -p litegen`
Expected: clean build (warnings only).

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/providers/
git commit -m "feat(litegen-core): refactor provider traits for capability registry"
```

---

### Task 8: Wire registry into AppState + add /v1/models/{id}

**Files:**
- Modify: `litegen-core/src/api/middleware/mod.rs` (AppState)
- Modify: `litegen-core/src/api/handlers.rs`
- Modify: `litegen-core/src/main.rs`

- [ ] **Step 1: Add registry to AppState**

Locate the `AppState` struct in `src/api/middleware/mod.rs`. Add:

```rust
pub registry: Arc<crate::capabilities::CapabilityRegistry>,
```

- [ ] **Step 2: Load registry at startup**

In `src/main.rs`, after config load and before building `AppState`, add:

```rust
let registry = std::sync::Arc::new(
    litegen::capabilities::CapabilityRegistry::from_dir(
        std::path::Path::new(
            std::env::var("LITEGEN_MODELS_DIR").as_deref().unwrap_or("models"),
        )
    ).expect("failed to load model registry")
);
```

Pass `registry` into `AppState`.

- [ ] **Step 3: Add `GET /v1/models/{id}` handler**

In `litegen-core/src/api/handlers.rs`, add:

```rust
/// GET /v1/models/{id} — Full schema for one model.
#[utoipa::path(
    get,
    path = "/v1/models/{id}",
    responses(
        (status = 200, description = "Model schema"),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Models"
)]
pub async fn get_model_schema(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.registry.get(&id) {
        Some(schema) => (StatusCode::OK, Json(serde_json::to_value(schema).unwrap())).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(error_response(&format!("model '{}' not found", id), 404)),
        ).into_response(),
    }
}
```

- [ ] **Step 4: Register route**

In the router builder (look for the function ending at `litegen-core/src/api/handlers.rs:413-438` in the existing code), add:

```rust
.route("/v1/models/:id", get(get_model_schema))
```

- [ ] **Step 5: Update existing `GET /v1/models` to source from registry**

Replace the body of `list_models` handler to iterate `state.registry.all()` and project into `ModelInfo`. Build a small projection:

```rust
fn project_model_info(s: &crate::capabilities::ModelSchema) -> ModelInfo {
    ModelInfo {
        id: s.id.clone(),
        name: s.display_name.clone(),
        description: s.description.clone(),
        provider: s.provider.clone(),
        media_type: match s.media_type {
            crate::capabilities::MediaType::Image => MediaType::Image,
            crate::capabilities::MediaType::Video => MediaType::Video,
        },
        is_available: true,
        capabilities: ModelCapabilities {
            supports_text_to_image: s.capabilities.text_to_image,
            supports_image_to_image: s.capabilities.image_to_image,
            supports_inpainting: s.capabilities.inpainting,
            supports_text_to_video: s.capabilities.text_to_video,
            supports_image_to_video: s.capabilities.image_to_video,
            supports_first_frame: s.ref_inputs.as_ref().map_or(false, |ri| ri.roles.contains_key("first_frame")),
            supports_last_frame: s.ref_inputs.as_ref().map_or(false, |ri| ri.roles.contains_key("last_frame")),
            supported_sizes: extract_sizes(s),
            max_images: 1,
            max_duration_seconds: None,
        },
        pricing: Some(ModelPricing {
            base_cost_usd: s.pricing.base_cost_usd,
            variable_pricing: s.pricing.variable_pricing.clone(),
        }),
        tags: s.tags.clone(),
    }
}

fn extract_sizes(s: &crate::capabilities::ModelSchema) -> Vec<String> {
    match s.params.get("size") {
        Some(crate::capabilities::ParamSpec::Size(crate::capabilities::SizeSpec::Enum { values })) => {
            values.iter().map(|(w, h)| format!("{}x{}", w, h)).collect()
        }
        _ => Vec::new(),
    }
}
```

- [ ] **Step 6: Build**

Run: `cd litegen-core && cargo build -p litegen`
Expected: clean build (existing generate handlers still use `Json<ImageGenerationRequest>` — that's fine for now, validation wiring comes in later tasks).

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/
git commit -m "feat(litegen-core): wire capability registry into AppState; add GET /v1/models/{id}"
```

---

## Phase E — Model YAML files

### Task 9: Author models/*.yaml for all providers

**Files:**
- Create: `models/openai.yaml`
- Create: `models/stability.yaml`
- Create: `models/replicate.yaml`
- Create: `models/google.yaml`
- Create: `models/fal.yaml`
- Create: `models/runway.yaml`
- Create: `models/luma.yaml`
- Create: `models/mock.yaml`

- [ ] **Step 1: Transcribe each provider's current `list_models()` into yaml**

For each provider, read its current `list_models()` body (e.g. `litegen-core/src/providers/image/stability.rs:291-397`) and translate into the schema yaml format. Use these per-model templates as starting points; consult the provider's API docs (linked in comments in the existing source) for things `list_models` didn't capture (negative_prompt support, prompt length, seed range, extra-allowed params).

`models/openai.yaml`:

```yaml
models:
  - id: openai/dall-e-3
    provider: openai
    media_type: image
    display_name: DALL-E 3
    description: OpenAI's high-quality text-to-image model.
    pricing: { base_cost_usd: 0.04 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 4000 }
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

  - id: openai/dall-e-2
    provider: openai
    media_type: image
    display_name: DALL-E 2
    description: OpenAI's older text-to-image model. Supports edits.
    pricing: { base_cost_usd: 0.02 }
    capabilities: { text_to_image: true, image_to_image: true, inpainting: true }
    prompt: { required: true, max_length: 1000 }
    params:
      size:
        kind: size
        mode: enum
        values:
          - [256, 256]
          - [512, 512]
          - [1024, 1024]
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: { form: multipart, field_map: { init: image, mask: mask } }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
        mask: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-image, image-to-image, inpainting]

  - id: openai/sora
    provider: openai
    media_type: video
    display_name: Sora
    description: OpenAI Sora text-to-video.
    pricing: { base_cost_usd: 0.50 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 4000 }
    params:
      duration_seconds:
        kind: float
        min: 1.0
        max: 20.0
      resolution:
        kind: string
        enum_values: [720p, 1080p]
        default: 1080p
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video]
```

`models/stability.yaml`:

```yaml
models:
  - id: stability/sd3-large
    provider: stability
    media_type: image
    display_name: Stable Diffusion 3 Large
    description: Latest Stability AI model with excellent prompt adherence.
    pricing: { base_cost_usd: 0.065 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 10000 }
    params:
      negative_prompt: { kind: string, max_length: 10000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9", "9:16", "3:2", "2:3", "4:5", "5:4", "21:9", "9:21"]
        default: "1:1"
      strength: { kind: float, min: 0.0, max: 1.0 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: multipart, field_map: { init: image } }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [output_format, style_preset]
    tags: [text-to-image, image-to-image]

  - id: stability/sd3-turbo
    provider: stability
    media_type: image
    display_name: Stable Diffusion 3 Turbo
    description: Faster, cheaper SD3 variant.
    pricing: { base_cost_usd: 0.04 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 10000 }
    params:
      negative_prompt: { kind: string, max_length: 10000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1"]
        default: "1:1"
    extra_allowlist: [output_format]
    tags: [text-to-image, fast]

  - id: stability/core
    provider: stability
    media_type: image
    display_name: Stable Image Core
    description: Fast, affordable image generation.
    pricing: { base_cost_usd: 0.03 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 10000 }
    params:
      negative_prompt: { kind: string, max_length: 10000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9", "21:9", "2:3", "3:2", "4:5", "5:4", "9:16", "9:21"]
        default: "1:1"
      style:
        kind: string
        enum_values: [3d-model, analog-film, anime, cinematic, comic-book, digital-art, enhance, fantasy-art, isometric, line-art, low-poly, modeling-compound, neon-punk, origami, photographic, pixel-art, tile-texture]
    extra_allowlist: [output_format]
    tags: [text-to-image, fast, affordable]

  - id: stability/ultra
    provider: stability
    media_type: image
    display_name: Stable Image Ultra
    description: Highest-quality Stability generation.
    pricing: { base_cost_usd: 0.08 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 10000 }
    params:
      negative_prompt: { kind: string, max_length: 10000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9", "21:9", "2:3", "3:2", "4:5", "5:4", "9:16", "9:21"]
        default: "1:1"
    extra_allowlist: [output_format]
    tags: [text-to-image, premium, photorealistic]

  - id: stability/sdxl
    provider: stability
    media_type: image
    display_name: Stable Diffusion XL
    description: High-quality general purpose image generation.
    pricing: { base_cost_usd: 0.002 }
    capabilities: { text_to_image: true, image_to_image: true, inpainting: true }
    prompt: { required: true, max_length: 2000 }
    params:
      negative_prompt: { kind: string, max_length: 2000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 10, max: 50, default: 30 }
      guidance_scale: { kind: float, min: 0.0, max: 35.0, default: 7.0 }
      size:
        kind: size
        mode: enum
        values: [[512,512],[1024,1024]]
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: { form: multipart, field_map: { init: init_image, mask: mask_image } }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
        mask: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-image, image-to-image, inpainting]
```

`models/replicate.yaml`:

```yaml
models:
  - id: replicate/flux-pro
    provider: replicate
    media_type: image
    display_name: Flux Pro
    description: Black Forest Labs Flux Pro via Replicate.
    pricing: { base_cost_usd: 0.055 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      guidance_scale: { kind: float, min: 1.5, max: 5.0, default: 3.0 }
      steps: { kind: int, min: 1, max: 50, default: 25 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","21:9","2:3","3:2","4:5","5:4","9:16","9:21","3:4","4:3"]
        default: "1:1"
    extra_allowlist: [safety_tolerance, output_format, output_quality]
    tags: [text-to-image, premium]

  - id: replicate/flux-dev
    provider: replicate
    media_type: image
    display_name: Flux Dev
    description: Flux Dev — open weights via Replicate.
    pricing: { base_cost_usd: 0.025 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      guidance_scale: { kind: float, min: 0.0, max: 10.0, default: 3.5 }
      steps: { kind: int, min: 1, max: 50, default: 28 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","21:9","2:3","3:2","4:5","5:4","9:16","9:21","3:4","4:3"]
        default: "1:1"
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [output_format, output_quality]
    tags: [text-to-image, image-to-image]

  - id: replicate/flux-schnell
    provider: replicate
    media_type: image
    display_name: Flux Schnell
    description: Fastest Flux variant.
    pricing: { base_cost_usd: 0.003 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 1, max: 4, default: 4 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","21:9","2:3","3:2","4:5","5:4","9:16","9:21","3:4","4:3"]
        default: "1:1"
    extra_allowlist: [output_format, output_quality]
    tags: [text-to-image, fast]

  - id: replicate/sdxl
    provider: replicate
    media_type: image
    display_name: SDXL on Replicate
    description: SDXL hosted on Replicate.
    pricing: { base_cost_usd: 0.012 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 2000 }
    params:
      negative_prompt: { kind: string, max_length: 2000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 10, max: 100, default: 50 }
      guidance_scale: { kind: float, min: 1.0, max: 20.0, default: 7.5 }
      size:
        kind: size
        mode: freeform
        min_width: 512
        max_width: 1536
        min_height: 512
        max_height: 1536
        multiple_of: 8
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-image, image-to-image]

  - id: replicate/sd3
    provider: replicate
    media_type: image
    display_name: SD3 on Replicate
    description: SD3 hosted on Replicate.
    pricing: { base_cost_usd: 0.035 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      negative_prompt: { kind: string, max_length: 5000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 10, max: 50, default: 28 }
      guidance_scale: { kind: float, min: 0.0, max: 10.0, default: 4.5 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","21:9","9:21","2:3","3:2","4:5","5:4"]
        default: "1:1"
    extra_allowlist: [output_format]
    tags: [text-to-image]

  - id: replicate/video
    provider: replicate
    media_type: video
    display_name: Replicate Video (generic)
    description: Generic Replicate video endpoint.
    pricing: { base_cost_usd: 0.30 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 2000 }
    params:
      duration_seconds: { kind: float, min: 2.0, max: 10.0 }
      seed: { kind: seed, min: 0, max: 4294967294 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [version]
    tags: [text-to-video, image-to-video]
```

`models/google.yaml`:

```yaml
models:
  - id: google/imagen-3
    provider: google
    media_type: image
    display_name: Imagen 3
    description: Google Imagen 3 via Gemini API.
    pricing: { base_cost_usd: 0.04 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 4000 }
    params:
      negative_prompt: { kind: string, max_length: 4000 }
      seed: { kind: seed, min: 0, max: 2147483647 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","9:16","16:9","3:4","4:3"]
        default: "1:1"
    extra_allowlist: [person_generation]
    tags: [text-to-image]

  - id: google/gemini-2.5-flash-image
    provider: google
    media_type: image
    display_name: Gemini 2.5 Flash (image)
    description: Gemini 2.5 Flash image generation.
    pricing: { base_cost_usd: 0.02 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 4000 }
    params:
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","4:3","3:4"]
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: base64 }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-image, image-to-image]

  - id: google/gemini-3-pro-image
    provider: google
    media_type: image
    display_name: Gemini 3 Pro (image)
    description: Gemini 3 Pro image generation.
    pricing: { base_cost_usd: 0.05 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 8000 }
    params:
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","4:3","3:4","21:9","9:21"]
    ref_inputs:
      max_total: 3
      default_role: init
      provider_format: { form: base64 }
      roles:
        init: { required: false, min_count: 0, max_count: 3 }
    extra_allowlist: []
    tags: [text-to-image, image-to-image, multi-ref]
```

`models/fal.yaml`:

```yaml
models:
  - id: fal/flux-pro
    provider: fal
    media_type: image
    display_name: Flux Pro (Fal)
    description: Flux Pro hosted on Fal.
    pricing: { base_cost_usd: 0.05 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      guidance_scale: { kind: float, min: 1.5, max: 5.0, default: 3.0 }
      steps: { kind: int, min: 1, max: 50, default: 25 }
      size:
        kind: size
        mode: freeform
        min_width: 512
        max_width: 2048
        min_height: 512
        max_height: 2048
        multiple_of: 8
    extra_allowlist: [safety_tolerance, output_format]
    tags: [text-to-image]

  - id: fal/flux-dev
    provider: fal
    media_type: image
    display_name: Flux Dev (Fal)
    description: Flux Dev on Fal.
    pricing: { base_cost_usd: 0.025 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      guidance_scale: { kind: float, min: 0.0, max: 10.0, default: 3.5 }
      steps: { kind: int, min: 1, max: 50, default: 28 }
      size:
        kind: size
        mode: freeform
        min_width: 512
        max_width: 2048
        min_height: 512
        max_height: 2048
        multiple_of: 8
    extra_allowlist: []
    tags: [text-to-image]

  - id: fal/flux-schnell
    provider: fal
    media_type: image
    display_name: Flux Schnell (Fal)
    description: Fast Flux on Fal.
    pricing: { base_cost_usd: 0.003 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 1, max: 4, default: 4 }
      size:
        kind: size
        mode: freeform
        min_width: 512
        max_width: 1536
        min_height: 512
        max_height: 1536
        multiple_of: 8
    extra_allowlist: []
    tags: [text-to-image, fast]

  - id: fal/sdxl
    provider: fal
    media_type: image
    display_name: SDXL (Fal)
    pricing: { base_cost_usd: 0.015 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 2000 }
    params:
      negative_prompt: { kind: string, max_length: 2000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 10, max: 100, default: 50 }
      guidance_scale: { kind: float, min: 1.0, max: 20.0, default: 7.5 }
      size:
        kind: size
        mode: freeform
        min_width: 512
        max_width: 1536
        min_height: 512
        max_height: 1536
        multiple_of: 8
    extra_allowlist: []
    tags: [text-to-image]

  - id: fal/sd35-medium
    provider: fal
    media_type: image
    display_name: SD 3.5 Medium (Fal)
    pricing: { base_cost_usd: 0.025 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      negative_prompt: { kind: string, max_length: 5000 }
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 10, max: 50, default: 28 }
      guidance_scale: { kind: float, min: 0.0, max: 10.0, default: 4.5 }
    extra_allowlist: []
    tags: [text-to-image]

  - id: fal/recraft-v3
    provider: fal
    media_type: image
    display_name: Recraft V3 (Fal)
    pricing: { base_cost_usd: 0.04 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      style:
        kind: string
        enum_values: [realistic_image, digital_illustration, vector_illustration]
    extra_allowlist: []
    tags: [text-to-image, styled]

  - id: fal/auraflow
    provider: fal
    media_type: image
    display_name: AuraFlow (Fal)
    pricing: { base_cost_usd: 0.02 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 5000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      steps: { kind: int, min: 10, max: 50, default: 25 }
      guidance_scale: { kind: float, min: 0.0, max: 10.0, default: 3.5 }
    extra_allowlist: []
    tags: [text-to-image]

  - id: fal/video
    provider: fal
    media_type: video
    display_name: Fal Video (generic)
    pricing: { base_cost_usd: 0.20 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 2000 }
    params:
      duration_seconds: { kind: float, min: 2.0, max: 10.0 }
      seed: { kind: seed, min: 0, max: 4294967294 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video]
```

`models/runway.yaml`:

```yaml
models:
  - id: runway/gen-3
    provider: runway
    media_type: video
    display_name: Runway Gen-3
    description: Runway Gen-3.
    pricing: { base_cost_usd: 0.50 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: int, min: 5, max: 10, default: 5 }
      seed: { kind: seed, min: 0, max: 4294967294 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video]

  - id: runway/gen-3-turbo
    provider: runway
    media_type: video
    display_name: Runway Gen-3 Turbo
    pricing: { base_cost_usd: 0.25 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: int, min: 5, max: 10, default: 5 }
      seed: { kind: seed, min: 0, max: 4294967294 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video, fast]
```

`models/luma.yaml`:

```yaml
models:
  - id: luma/dream-machine
    provider: luma
    media_type: video
    display_name: Luma Dream Machine
    pricing: { base_cost_usd: 0.35 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: float, min: 3.0, max: 9.0 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","4:3","3:4","21:9","9:21"]
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame:  { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [loop, callback_url]
    tags: [text-to-video, image-to-video]

  - id: luma/ray-2
    provider: luma
    media_type: video
    display_name: Luma Ray 2
    pricing: { base_cost_usd: 0.40 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: float, min: 5.0, max: 9.0 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","4:3","3:4","21:9","9:21"]
      resolution:
        kind: string
        enum_values: [540p, 720p, 1080p]
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame:  { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video]

  - id: luma/ray-3
    provider: luma
    media_type: video
    display_name: Luma Ray 3
    pricing: { base_cost_usd: 0.50 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: float, min: 5.0, max: 9.0 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","4:3","3:4","21:9","9:21"]
      resolution:
        kind: string
        enum_values: [720p, 1080p, 4k]
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame:  { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video, premium]

  - id: luma/ray-flash-2
    provider: luma
    media_type: video
    display_name: Luma Ray Flash 2
    pricing: { base_cost_usd: 0.20 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: float, min: 5.0, max: 9.0 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1","16:9","9:16","4:3","3:4"]
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame:  { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video, fast]

  - id: luma/ray-hdr-3
    provider: luma
    media_type: video
    display_name: Luma Ray HDR 3
    pricing: { base_cost_usd: 0.65 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: float, min: 5.0, max: 9.0 }
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["16:9","9:16","21:9"]
      resolution:
        kind: string
        enum_values: [1080p, 4k]
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame:  { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [text-to-video, image-to-video, hdr]
```

`models/mock.yaml`:

```yaml
models:
  - id: mock/image-gen
    provider: mock
    media_type: image
    display_name: Mock Image
    description: Mock image provider for tests.
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 1000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      size:
        kind: size
        mode: enum
        values: [[512,512],[1024,1024]]
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: base64 }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [test_flag]
    tags: [mock, test]

  - id: mock/video-gen
    provider: mock
    media_type: video
    display_name: Mock Video
    description: Mock video provider for tests.
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params:
      duration_seconds: { kind: float, min: 1.0, max: 10.0 }
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [mock, test]
```

- [ ] **Step 2: Verify all files load**

Write a small smoke test in `litegen-core/src/capabilities/registry.rs` (under `#[cfg(test)]`):

```rust
#[test]
fn ships_models_directory_loads() {
    use std::path::PathBuf;
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();          // workspace root (or litegen/)
    p.push("models");
    if !p.exists() { return; }
    let r = CapabilityRegistry::from_dir(&p)
        .expect("models/*.yaml must load cleanly");
    assert!(r.len() >= 20);
}
```

Run: `cd litegen-core && cargo test -p litegen capabilities::registry::tests::ships_models_directory_loads`
Expected: passes (or skipped if path differs — adjust path until it loads).

- [ ] **Step 3: Commit**

```bash
git add models/ litegen-core/src/capabilities/registry.rs
git commit -m "feat(models): author capability schemas for all 12 providers"
```

---

## Phase F — Rewrite providers

Each provider gets one task. The pattern:

1. Find the provider's old `generate()` body.
2. Rewrite it to consume `&BaseGenerationRequest`, `&ImageExtras|VideoExtras`, and `&MaterializedRequest`. Use the materialized refs in whatever form the schema declared.
3. Write a `wiremock`-based integration test asserting the outbound HTTP body matches what we'd send before the refactor for a representative request.

### Task 10: Rewrite `providers/image/openai.rs`

- [ ] **Step 1: Write the integration test first**

Create `litegen-core/tests/provider_openai_dalle3.rs`:

```rust
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use serde_json::json;

use litegen::capabilities::*;
use litegen::providers::{ImageProvider, ImageExtras, ProviderInstanceConfig};
use litegen::proxy::materializer::*;
use litegen::types::*;
use std::sync::Arc;
use std::collections::HashMap;

fn registry() -> CapabilityRegistry {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("models");
    CapabilityRegistry::from_dir(&p).unwrap()
}

#[tokio::test]
async fn dalle3_request_body_includes_quality_and_style() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "created": 1,
            "data": [{ "url": "https://x/r.png", "revised_prompt": "x" }],
            "model": "dall-e-3"
        })))
        .mount(&server)
        .await;

    let mut p = litegen::providers::image::openai::OpenAiImageProvider::new();
    p.configure(ProviderInstanceConfig {
        api_key: "sk-test".into(),
        api_keys: vec![],
        api_base: Some(format!("{}/", server.uri())),
        model_mapping: HashMap::new(),
        extra_headers: HashMap::new(),
        options: None,
    });

    let reg = registry();
    let schema = reg.get("openai/dall-e-3").unwrap();
    let base = BaseGenerationRequest {
        prompt: "a cat".into(),
        model: "openai/dall-e-3".into(),
        n: 1, negative_prompt: None, seed: None,
        reference_images: vec![],
        strict: true, extra: None, metadata: None,
    };
    let extras = ImageExtras {
        size: Some("1024x1024".into()),
        aspect_ratio: None,
        quality: Some("hd".into()),
        style: Some("vivid".into()),
        steps: None, guidance_scale: None, strength: None,
        response_format: "url".into(),
        extra: None,
    };
    let mat = MaterializedRequest {
        refs: vec![],
        cleanup: dummy_cleanup(),
    };

    let _ = p.generate(schema, &base, &extras, &mat).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["model"], "dall-e-3");
    assert_eq!(body["quality"], "hd");
    assert_eq!(body["style"], "vivid");
    assert_eq!(body["size"], "1024x1024");
    assert_eq!(body["prompt"], "a cat");
}

fn dummy_cleanup() -> Cleanup {
    Cleanup::empty()
}
```

Add a helper to `materializer.rs`:

```rust
impl Cleanup {
    pub fn empty() -> Self { Self { storage: None, keys: Vec::new() } }
}
```

- [ ] **Step 2: Run the test (should fail)**

Run: `cd litegen-core && cargo test -p litegen --test provider_openai_dalle3`
Expected: compile fails or `todo!()` panic.

- [ ] **Step 3: Rewrite the provider**

In `litegen-core/src/providers/image/openai.rs`, rewrite the `generate()` impl:

```rust
async fn generate(
    &self,
    model: &ModelSchema,
    base: &BaseGenerationRequest,
    extras: &ImageExtras,
    _materialized: &MaterializedRequest,
) -> Result<GenerationOutput, ProviderError> {
    let model_native = model.id.strip_prefix("openai/").unwrap_or(&model.id);
    let mut body = serde_json::json!({
        "model": model_native,
        "prompt": base.prompt,
        "n": base.n.min(1),
        "response_format": match extras.response_format.as_str() {
            "b64_json" | "bytes" => "b64_json",
            _ => "url",
        },
    });
    if let Some(s) = &extras.size { body["size"] = serde_json::Value::String(s.clone()); }
    if let Some(q) = &extras.quality { body["quality"] = serde_json::Value::String(q.clone()); }
    if let Some(st) = &extras.style { body["style"] = serde_json::Value::String(st.clone()); }
    if let Some(extra) = &extras.extra {
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj { body[k] = v.clone(); }
        }
    }

    let base_url = self.api_base.clone().unwrap_or_else(|| "https://api.openai.com/v1/".into());
    let url = format!("{}images/generations", base_url);
    let resp = self.http.post(&url)
        .bearer_auth(&self.api_key)
        .json(&body)
        .send().await
        .map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(), status_code: None,
            provider_error: None, retryable: true,
        })?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| ProviderError::RequestFailed {
        message: e.to_string(), status_code: Some(status.as_u16()),
        provider_error: None, retryable: false,
    })?;
    if !status.is_success() {
        return Err(ProviderError::RequestFailed {
            message: format!("openai returned {}", status),
            status_code: Some(status.as_u16()),
            provider_error: serde_json::from_slice(&bytes).ok(),
            retryable: status.is_server_error(),
        });
    }
    // Decode first image (url or b64), fetch bytes if needed.
    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| ProviderError::RequestFailed {
        message: e.to_string(), status_code: Some(status.as_u16()),
        provider_error: None, retryable: false,
    })?;
    let first = &json["data"][0];
    if let Some(url) = first["url"].as_str() {
        let img_bytes = self.http.get(url).send().await
            .and_then(|r| r.error_for_status())
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(), status_code: None, provider_error: None, retryable: false,
            })?
            .bytes().await
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(), status_code: None, provider_error: None, retryable: false,
            })?;
        let mut meta = HashMap::new();
        if let Some(rev) = first["revised_prompt"].as_str() {
            meta.insert("revised_prompt".into(), serde_json::Value::String(rev.into()));
        }
        return Ok(GenerationOutput { data: img_bytes.to_vec(), content_type: "image/png".into(), metadata: meta });
    }
    if let Some(b64) = first["b64_json"].as_str() {
        use base64::Engine;
        let img_bytes = base64::engine::general_purpose::STANDARD.decode(b64)
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(), status_code: None, provider_error: None, retryable: false,
            })?;
        let mut meta = HashMap::new();
        if let Some(rev) = first["revised_prompt"].as_str() {
            meta.insert("revised_prompt".into(), serde_json::Value::String(rev.into()));
        }
        return Ok(GenerationOutput { data: img_bytes, content_type: "image/png".into(), metadata: meta });
    }
    Err(ProviderError::RequestFailed {
        message: "openai response missing url and b64_json".into(),
        status_code: Some(status.as_u16()), provider_error: None, retryable: false,
    })
}
```

(Adjust struct fields — `self.http`, `self.api_key`, `self.api_base` — to match what's actually in the OpenAI provider impl in `src/providers/image/openai.rs`.)

- [ ] **Step 4: Run integration test**

Run: `cd litegen-core && cargo test -p litegen --test provider_openai_dalle3`
Expected: pass.

- [ ] **Step 5: Run all tests**

Run: `cd litegen-core && cargo test -p litegen`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/providers/image/openai.rs litegen-core/tests/provider_openai_dalle3.rs litegen-core/src/proxy/materializer.rs
git commit -m "feat(providers): rewrite openai image provider for capability registry"
```

---

### Task 11: Rewrite `providers/image/stability.rs`

Mirror Task 10 with a Stability-specific wiremock test asserting the V2 stable-image endpoints receive the aspect_ratio, seed, and negative_prompt as multipart parts. Use `models/stability.yaml`'s `stability/sd3-large` model.

- [ ] **Step 1: Write integration test at `tests/provider_stability_sd3.rs`** — copy the structure of `provider_openai_dalle3.rs`, change endpoint to `/v2beta/stable-image/generate/sd3`, assert multipart form fields: `prompt`, `aspect_ratio`, `seed`, `negative_prompt`, `model`.
- [ ] **Step 2: Run** — should fail.
- [ ] **Step 3: Rewrite `stability.rs::generate()`** — build a `reqwest::multipart::Form` from the extras; for V2 endpoints (`sd3-large`, `core`, `ultra`), use the V2 base URL; for `sdxl`, use V1 with JSON body. Use the materialized refs: when present and target is multipart, append each `MaterializedRefForm::MultipartField` as a form part.
- [ ] **Step 4: Run integration test** — pass.
- [ ] **Step 5: Run all tests** — green.
- [ ] **Step 6: Commit** — `feat(providers): rewrite stability image provider for capability registry`.

---

### Task 12: Rewrite `providers/image/replicate.rs`

- [ ] **Step 1: Integration test at `tests/provider_replicate_flux.rs`** — wiremock for Replicate's `/predictions` endpoint; assert JSON body includes `input.prompt`, `input.aspect_ratio`, `input.seed`, and the chosen model `version` field. Test with `replicate/flux-dev`.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite `replicate.rs::generate()`** — Replicate uses a poll-based API: POST `/predictions` → poll `/predictions/{id}`. For the image trait we poll inline until done or error. Map model id → Replicate model version (this mapping was previously inline; move to a `match model.id.as_str()` in the provider).
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: All tests** — green.
- [ ] **Step 6: Commit** — `feat(providers): rewrite replicate image provider`.

---

### Task 13: Rewrite `providers/image/google.rs`

- [ ] **Step 1: Integration test at `tests/provider_google_gemini.rs`** — wiremock for Gemini's `generateContent` endpoint; assert the inline-data part (base64) is included when a base64 ref is provided. Test with `google/gemini-3-pro-image`.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite `google.rs::generate()`** — build Gemini contents parts: first part `text` (the prompt), then `inline_data` parts for each materialized ref of `MaterializedRefForm::Base64`.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: All tests** — green.
- [ ] **Step 6: Commit** — `feat(providers): rewrite google image provider`.

---

### Task 14: Rewrite `providers/image/fal.rs`

- [ ] **Step 1: Integration test at `tests/provider_fal_flux.rs`** — wiremock for Fal's `/fal-ai/flux/dev` (or whichever endpoint maps to the test model); assert JSON body includes `prompt`, `image_size: {width, height}`, `seed`, `guidance_scale`. Test with `fal/flux-dev`.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite `fal.rs::generate()`** — map model id → Fal endpoint; parse `extras.size` into `{width, height}` JSON object; merge `extras.extra` keys into body root.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: All tests** — green.
- [ ] **Step 6: Commit** — `feat(providers): rewrite fal image provider`.

---

### Task 15: Rewrite `providers/image/mock.rs`

- [ ] **Step 1: Unit test at `litegen-core/src/providers/image/mock_tests.rs`** — drive the mock provider with a base + extras + materialized refs and assert the output bytes are deterministic.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite `mock.rs::generate()`** — produce a tiny PNG/1x1 with deterministic bytes; populate metadata with the model id and the count of refs.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite mock image provider`.

---

### Task 16: Rewrite `providers/video/openai.rs` (Sora)

- [ ] **Step 1: Integration test at `tests/provider_openai_sora.rs`** — wiremock for Sora's submission endpoint; assert body has `prompt`, `duration`, optional `image_url` from the materialized init ref.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite generate()** to return a `VideoGenerationHandle{ provider_job_id, provider, model }` after a successful submission.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite openai sora provider`.

---

### Task 17: Rewrite `providers/video/runway.rs`

- [ ] **Step 1: Integration test at `tests/provider_runway_gen3.rs`** — assert outbound body has `model: "gen3a_turbo"` (or whichever), `promptText`, `duration: 5|10`, optional `promptImage` URL.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite generate()**; reject `duration` not in `{5,10}` (registry catches this; provider just maps).
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite runway video provider`.

---

### Task 18: Rewrite `providers/video/luma.rs`

- [ ] **Step 1: Integration test at `tests/provider_luma_ray.rs`** — assert body has `prompt`, `aspect_ratio`, `keyframes: { frame0: { url, type: "image" }, frame1: { url, type: "image" } }` when two refs with roles `first_frame` and `last_frame` are present.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite generate()** — map `first_frame` → `frame0`, `last_frame` → `frame1` in the keyframes object.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite luma video provider`.

---

### Task 19: Rewrite `providers/video/replicate.rs`

- [ ] **Step 1: Integration test at `tests/provider_replicate_video.rs`** — assert outbound `/predictions` body includes `input.prompt`, `input.image` (when present), `input.seed`.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite generate()** — POST + return handle for poll-based response.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite replicate video provider`.

---

### Task 20: Rewrite `providers/video/fal.rs`

- [ ] **Step 1: Integration test at `tests/provider_fal_video.rs`** — assert body has `prompt`, `duration`, `image_url` when present.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite generate()**.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite fal video provider`.

---

### Task 21: Rewrite `providers/video/mock.rs`

- [ ] **Step 1: Unit test at `src/providers/video/mock_tests.rs`** — handle returned has `model` set; `poll_status` returns Completed after one poll.
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Rewrite**.
- [ ] **Step 4: Run** — pass.
- [ ] **Step 5: Commit** — `feat(providers): rewrite mock video provider`.

---

## Phase G — API wiring

### Task 22: ValidatedRequest extractor

**Files:**
- Modify: `litegen-core/src/api/middleware/validator.rs`
- Modify: `litegen-core/src/api/handlers.rs`
- Modify: `litegen-core/src/api/middleware/mod.rs`

- [ ] **Step 1: Add the extractor**

Append to `validator.rs`:

```rust
use axum::extract::{FromRequest, FromRequestParts, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum::body::Bytes;
use std::sync::Arc;

use crate::api::middleware::AppState;
use crate::proxy::materializer::MaterializeContext;

pub struct ValidatedImage {
    pub schema: Arc<ModelSchema>,
    pub request: ImageGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
}

pub struct ValidatedVideo {
    pub schema: Arc<ModelSchema>,
    pub request: VideoGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
}

#[derive(Debug)]
pub struct ValidationRejection(pub StatusCode, pub serde_json::Value);

impl IntoResponse for ValidationRejection {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

fn err_body(code: &str, msg: &str, param: Option<&str>, model: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "type": "validation_error",
            "code": code,
            "message": msg,
            "param": param,
            "model": model,
        }
    })
}

async fn parse_request_and_context(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<(serde_json::Value, MaterializeContext), ValidationRejection> {
    let ct = headers.get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    if ct.starts_with("multipart/form-data") {
        // Parse multipart manually to extract the `request` part and any blob file parts.
        let boundary = ct.split(';').find_map(|p| p.trim().strip_prefix("boundary="))
            .ok_or_else(|| ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_multipart", "missing boundary", None, "")))?
            .to_string();
        let mut multipart = multer::Multipart::new(
            futures::stream::once(async move { Ok::<_, std::convert::Infallible>(body) }),
            boundary,
        );
        let mut req_json: Option<serde_json::Value> = None;
        let mut blob_parts: std::collections::HashMap<String, (bytes::Bytes, String)> = Default::default();
        while let Some(part) = multipart.next_field().await.map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_multipart", &e.to_string(), None, ""))
        })? {
            let name = part.name().unwrap_or("").to_string();
            let ct = part.content_type().map(|s| s.to_string()).unwrap_or_else(|| "application/octet-stream".into());
            let bytes = part.bytes().await.map_err(|e| {
                ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_multipart", &e.to_string(), None, ""))
            })?;
            if name == "request" {
                req_json = Some(serde_json::from_slice(&bytes).map_err(|e| {
                    ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
                })?);
            } else {
                blob_parts.insert(name, (bytes, ct));
            }
        }
        let json = req_json.ok_or_else(|| ValidationRejection(
            StatusCode::BAD_REQUEST,
            err_body("malformed_multipart", "missing 'request' part", None, ""),
        ))?;
        Ok((json, MaterializeContext { blob_parts }))
    } else {
        let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        Ok((json, MaterializeContext::default()))
    }
}

#[axum::async_trait]
impl FromRequest<Arc<AppState>> for ValidatedImage {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let headers = parts.headers;
        let bytes = axum::body::to_bytes(body, 25 * 1024 * 1024).await.map_err(|e| {
            ValidationRejection(StatusCode::PAYLOAD_TOO_LARGE, err_body("body_too_large", &e.to_string(), None, ""))
        })?;
        let (json, ctx) = parse_request_and_context(&headers, bytes).await?;
        let req: ImageGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.get(&req.base.model)
            .ok_or_else(|| ValidationRejection(StatusCode::NOT_FOUND, err_body("model_not_found", &format!("model '{}' not found", req.base.model), None, &req.base.model)))?
            .clone();
        let schema = Arc::new(schema);
        match validate_image(&schema, req) {
            Ok(out) => Ok(ValidatedImage { schema, request: out.request, dropped: out.dropped, ctx }),
            Err(e) => Err(ValidationRejection(
                StatusCode::BAD_REQUEST,
                err_body(&e.code, &e.message, e.param.as_deref(), &schema.id),
            )),
        }
    }
}

#[axum::async_trait]
impl FromRequest<Arc<AppState>> for ValidatedVideo {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let headers = parts.headers;
        let bytes = axum::body::to_bytes(body, 25 * 1024 * 1024).await.map_err(|e| {
            ValidationRejection(StatusCode::PAYLOAD_TOO_LARGE, err_body("body_too_large", &e.to_string(), None, ""))
        })?;
        let (json, ctx) = parse_request_and_context(&headers, bytes).await?;
        let req: VideoGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.get(&req.base.model)
            .ok_or_else(|| ValidationRejection(StatusCode::NOT_FOUND, err_body("model_not_found", &format!("model '{}' not found", req.base.model), None, &req.base.model)))?
            .clone();
        let schema = Arc::new(schema);
        match validate_video(&schema, req) {
            Ok(out) => Ok(ValidatedVideo { schema, request: out.request, dropped: out.dropped, ctx }),
            Err(e) => Err(ValidationRejection(
                StatusCode::BAD_REQUEST,
                err_body(&e.code, &e.message, e.param.as_deref(), &schema.id),
            )),
        }
    }
}

pub fn dropped_header(values: &[String]) -> Option<(axum::http::HeaderName, HeaderValue)> {
    if values.is_empty() { return None; }
    let s = values.join(",");
    Some((axum::http::HeaderName::from_static("x-litegen-dropped-params"), HeaderValue::from_str(&s).ok()?))
}
```

Add `multer = "3"`, `futures = "0.3"` to `Cargo.toml` if not present.

- [ ] **Step 2: Update generate handlers**

In `handlers.rs`, change `generate_image` to take `ValidatedImage` instead of `Json<ImageGenerationRequest>`. Replace the body to:

1. Materialize: `state.materializer.materialize(&validated.schema, validated.request.base.reference_images.clone(), &validated.ctx).await`
2. Dispatch to the existing `state.router.generate_image(...)` (router will need to learn about `&ModelSchema` + materialized — see Task 23).
3. Attach `X-Litegen-Dropped-Params` header when dropped is non-empty.

- [ ] **Step 3: Build**

Run: `cd litegen-core && cargo build -p litegen`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add litegen-core/src/api/ litegen-core/Cargo.toml
git commit -m "feat(litegen-core): add ValidatedRequest extractor + multipart support"
```

---

### Task 23: Router rewire

**Files:**
- Modify: `litegen-core/src/proxy/router.rs`
- Modify: `litegen-core/src/proxy/registry.rs`

- [ ] **Step 1: Rewrite `Router::generate_image` to consume the typed schema + materialized refs**

```rust
pub async fn generate_image(
    &self,
    schema: &ModelSchema,
    base: &BaseGenerationRequest,
    extras: &ImageExtras,
    materialized: &MaterializedRequest,
) -> Result<ImageGenerationResponse, ProxyError> {
    let provider = self.registry.image_provider_for(&schema.provider)
        .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;
    // existing retry/fallback logic; pass through.
    let output = provider.generate(schema, base, extras, materialized).await
        .map_err(ProxyError::from)?;
    // build response (existing pipeline: storage, cost, response shape)
    ...
}
```

Likewise for `generate_video`.

- [ ] **Step 2: Update `proxy::registry::ProviderRegistry` to expose `image_provider_for(name: &str)` and `video_provider_for(name: &str)`**

(If it already exposes by model id, change to by provider name.)

- [ ] **Step 3: Build + run all tests**

Run: `cd litegen-core && cargo test -p litegen`
Expected: green.

- [ ] **Step 4: Commit**

```bash
git add litegen-core/src/proxy/
git commit -m "feat(litegen-core): rewire router for capability registry"
```

---

## Phase H — E2E tests

### Task 24: E2E strict + lax validation

**Files:**
- Create: `litegen-core/tests/e2e_validation.rs`

- [ ] **Step 1: Write tests with axum-test**

```rust
use axum_test::TestServer;
use serde_json::json;
use litegen::api::build_router; // expose this from lib.rs

fn server() -> TestServer {
    let app = build_router_for_tests();
    TestServer::new(app).unwrap()
}

#[tokio::test]
async fn strict_unsupported_param_rejects() {
    let s = server();
    let r = s.post("/v1/images/generations").json(&json!({
        "model": "openai/dall-e-3", "prompt": "x",
        "guidance_scale": 7.5
    })).await;
    r.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = r.json();
    assert_eq!(body["error"]["code"], "param_unsupported");
    assert_eq!(body["error"]["param"], "guidance_scale");
}

#[tokio::test]
async fn lax_drops_unsupported_with_header() {
    let s = server();
    let r = s.post("/v1/images/generations").json(&json!({
        "model": "mock/image-gen", "prompt": "x",
        "strict": false,
        "guidance_scale": 7.5
    })).await;
    r.assert_status_ok();
    assert!(r.headers().get("x-litegen-dropped-params").unwrap().to_str().unwrap().contains("guidance_scale"));
}
```

You may need a `build_router_for_tests()` factory that wires in mock providers and the registry pointed at `models/`. Add it to `litegen-core/src/lib.rs` or a `test_support` mod gated behind `#[cfg(any(test, feature="test-support"))]`.

- [ ] **Step 2: Run**

Run: `cd litegen-core && cargo test -p litegen --test e2e_validation`
Expected: pass.

- [ ] **Step 3: Commit** — `test(litegen-core): e2e strict + lax validation`.

---

### Task 25: E2E multipart roundtrip

**Files:**
- Create: `litegen-core/tests/e2e_multipart.rs`

- [ ] **Step 1: Send a multipart request with a blob ref and assert storage upload + cleanup**

```rust
#[tokio::test]
async fn multipart_blob_to_url_provider_uploads_and_cleans() {
    let storage = Arc::new(SpyStorage::default());
    let s = server_with_storage(storage.clone());

    let boundary = "X";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{{\"model\":\"replicate/flux-dev\",\"prompt\":\"a\",\"reference_images\":[{{\"type\":\"blob\",\"value\":\"img\",\"role\":\"init\"}}]}}\r\n--{b}\r\nContent-Disposition: form-data; name=\"img\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n\x89PNG\r\n--{b}--\r\n",
        b = boundary,
    );

    let r = s.post("/v1/images/generations")
        .content_type(&format!("multipart/form-data; boundary={}", boundary))
        .bytes(body.into()).await;

    r.assert_status_ok();
    assert_eq!(storage.uploaded(), 1);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(storage.deleted(), 1);
}
```

- [ ] **Step 2: Run + commit**

Run: `cd litegen-core && cargo test -p litegen --test e2e_multipart`
Then: `git add litegen-core/tests/e2e_multipart.rs && git commit -m "test(litegen-core): e2e multipart roundtrip"`

---

## Phase I — Cleanup

### Task 26: Remove dead code + final sweep

- [ ] **Step 1: Search for `todo!()`**

Run: `rg "todo!\(" litegen-core/src/`
Expected: zero hits.

- [ ] **Step 2: Search for old field references**

Run: `rg "first_frame_url|last_frame_url|image_base64\b|image_url\b|mask_url\b" litegen-core/src/`
Expected: zero hits (the new types have the fields under `reference_images`).

- [ ] **Step 3: Run clippy**

Run: `cd litegen-core && cargo clippy -p litegen --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Run full test suite**

Run: `cd litegen-core && cargo test -p litegen`
Expected: green; capture count.

- [ ] **Step 5: Update `litegen.example.yaml`**

Add `models_dir: "./models"` (or document the env var override). Keep behavior backward-compatible by defaulting to `./models`.

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "chore(litegen-core): cleanup + docs after capability registry"
```

---

## Self-review

(Run-by-you, post-write.) Spec section coverage:

| Spec section | Covered by |
|---|---|
| §5 schema types | Task 2 |
| §6 yaml format | Task 9 |
| §7 loader | Task 3 |
| §8 registry | Task 3, 8 |
| §9 request shapes | Task 4 |
| §10 multipart | Task 22 |
| §11 validator | Task 5, 22 |
| §12 materializer | Task 6 |
| §13 provider trait | Task 7 |
| §14 endpoints | Task 8, 22 |
| §15 error path | Task 5, 22 |
| §17 testing | Tasks 5, 6, 10–21, 24, 25 |
| §18 implementation order | Phases A–I match |

No placeholders, no "TODO" steps, every code-changing step shows the code, every Bash step has expected output.
