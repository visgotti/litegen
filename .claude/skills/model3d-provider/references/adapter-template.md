# 3D adapter template

The reference implementation is `litegen-core/src/providers/model3d/fal.rs`.
It was written after the 3D contract settled, and its comments explain every
vendor trap it handles. Its committed version was proven with real billed
runs on 2026-09-12. A rewrite was in progress on 2026-10-12, so re-run
`tests/live_model3d.rs` before relying on a changed path.
Read it first. This file condenses it into a vendor-neutral skeleton, with
the pieces every 3D adapter needs and the places where vendors differ.
"Acme" is a placeholder.

## Contents

- [Adapter skeleton](#adapter-skeleton)
- [Tests](#tests)
- [Catalog row](#catalog-row)
- [Live structural checks](#live-structural-checks)

## Adapter skeleton

```rust
//! Acme 3D (mesh) generation provider.
//!
//! Async: POST {api_base}/v1/{endpoint} → {task_id}; GET {api_base}/v1/tasks/{task_id}
//! until status is terminal; the result lists per-format mesh URLs that EXPIRE
//! ~5 min after success, so they are downloaded inside poll_status.
//!
//! @see <https://docs.acme.example/3d/quick-start>        — submit/poll, auth
//!   Verbatim: "Authorization: Bearer <token>"
//!   Verbatim: "Model URLs expire after 5 minutes. Download immediately after the task succeeds."
//! @see <https://docs.acme.example/3d/errors>             — error envelope
//! @see <https://docs.acme.example/3d/openapi.json> (2026-10-12) — every request/response type

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, HealthCheckResult, Model3dExtras,
    Model3dFile, Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider,
    ProviderError, ProviderInstanceConfig,
};
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::types::*;

/// Bytes are held in memory through rehost, and `mesh::MAX_INPUT_BYTES`
/// refuses to parse anything larger anyway.
const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;

/// What ONE endpoint's schema accepts. Read every field from that endpoint's
/// own schema. Vendors (aggregators especially) don't share a vocabulary.
struct EndpointSpec {
    text_to_3d: &'static str,          // "" = no text mode → InvalidRequest without a ref
    image_to_3d: &'static str,         // "" = no image mode
    image_field: &'static str,         // spelling differs per endpoint
    image_is_array: bool,
    format_field: &'static str,        // "" = emits a fixed container; send NO field
    format_values: &'static [&'static str],
    seed_max: Option<i64>,             // None = endpoint takes no seed
    texture_field: &'static str,
}

const UNSUPPORTED: EndpointSpec = EndpointSpec {
    text_to_3d: "", image_to_3d: "", image_field: "", image_is_array: false,
    format_field: "", format_values: &[], seed_max: None, texture_field: "",
};

pub struct AcmeModel3dProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl AcmeModel3dProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder().timeout(std::time::Duration::from_secs(300)).build().expect("http client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config.as_ref().and_then(|c| c.api_base.as_deref()).unwrap_or("https://api.acme.example")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> { /* as in auth-and-credentials.md */ todo!() }

    /// One arm per model3d row in models/acme.yaml.
    fn resolve_spec(model: &str) -> EndpointSpec {
        match model {
            // @see <https://docs.acme.example/3d/models/mesh-2> — API page
            // @see <https://docs.acme.example/3d/openapi.json> (2026-10-12) — Mesh2Input / Mesh2Output
            // @see <https://acme.example/pricing.md> (2026-10-12) — "20 credits ($0.20) per generation"
            "acme/mesh-2" => EndpointSpec {
                text_to_3d: "generation/text-to-model",
                image_to_3d: "generation/image-to-model",
                image_field: "image_url",
                image_is_array: false,
                format_field: "output_format",                 // Mesh2Input.output_format (scalar)
                format_values: &["glb", "fbx", "usdz"],
                seed_max: Some(i64::from(u32::MAX)),
                texture_field: "texture",
            },
            _ => UNSUPPORTED,
        }
    }

    /// Ask the vendor for exactly ONE container (see SKILL.md, "The output_formats promise").
    fn vendor_format(spec: &EndpointSpec, requested: &[String]) -> Option<String> {
        if spec.format_values.is_empty() {
            return None;
        }
        let offers = |f: &str| spec.format_values.iter().any(|v| v.eq_ignore_ascii_case(f));
        // Never test derivability directly: the seam also keeps fbx/usdz vendor-made.
        if let Some(native_only) = requested.iter().find(|r| crate::proxy::model3d_convert::prefers_vendor_native(r) && offers(r)) {
            return Some(native_only.clone());                   // fbx, usdz, or anything litegen can't derive
        }
        if offers(DEFAULT_MODEL3D_FORMAT) {
            return Some(DEFAULT_MODEL3D_FORMAT.to_string());    // glb: the conversion source
        }
        requested.iter().find(|r| offers(r)).cloned().or_else(|| spec.format_values.first().map(|s| s.to_string()))
    }

    /// Downloads one vendor asset, size-capped, with no auth header on CDN
    /// URLs. Shape it like `fal.rs` `fetch_asset`:
    /// - `crate::util::ssrf::validate_public_url` first (behind a
    ///   `public_assets_only` switch that tests turn off for wiremock);
    /// - a no-redirect client (`util::ssrf::no_redirect_client_with_timeout`);
    /// - a Content-Length precheck, a streamed read against the cap, and the
    ///   status check.
    ///
    /// It returns `AssetError` so the caller can tell "ask again later" from
    /// "this will never download".
    async fn fetch_asset(&self, url: &str) -> Result<(Vec<u8>, String), AssetError> {
        todo!()
    }
}

/// Why an asset didn't download.
enum AssetError {
    /// Asking again may work: a dropped connection, or a 5xx / 408 / 429.
    Transient(ProviderError),
    /// It never will: gone, expired, refused, private or too large.
    Gone(String),
}

/// Container from the URL's extension (object stores label everything
/// octet-stream), then the content type, then glb. See fal.rs `format_of`.
fn format_of(url: &str, content_type: &str) -> String { todo!() }

#[async_trait]
impl Model3dProvider for AcmeModel3dProvider {
    fn name(&self) -> &str { "acme" }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config.as_ref().is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let spec = Self::resolve_spec(&model.id);

        // Mode from refs. A ref we can't forward is refused, never silently
        // dropped. A `Url` form may hold an inline `data:` URI when the caller
        // uploaded bytes (see known-gaps.md). If Acme can't take data URIs,
        // refuse those too, or upload them to Acme's file API instead.
        let images: Vec<String> = materialized.refs.iter().filter(|r| r.role == "init").map(|r| match &r.form {
            MaterializedRefForm::Url(u) if !u.starts_with("data:") => Ok(u.clone()),
            _ => Err(ProviderError::InvalidRequest(format!("{}: the init image must be a public http(s) URL", model.id))),
        }).collect::<Result<_, _>>()?;
        let endpoint = match (images.is_empty(), spec.text_to_3d, spec.image_to_3d) {
            (true, "", _) => return Err(ProviderError::InvalidRequest(format!("'{}' is image-to-3D only; supply an init image", model.id))),
            (true, t, _) => t,
            (false, _, "") => return Err(ProviderError::InvalidRequest(format!("'{}' does not accept a reference image", model.id))),
            (false, _, i) => i,
        };

        // extra FIRST; everything we own is written after it.
        let mut body = match &extras.extra { Some(Value::Object(m)) => Value::Object(m.clone()), _ => json!({}) };
        if !base.prompt.trim().is_empty() { body["prompt"] = json!(base.prompt); }
        if !images.is_empty() {
            body[spec.image_field] = if spec.image_is_array { json!(images) } else { json!(images[0]) };
        }
        if let Some(fmt) = Self::vendor_format(&spec, &extras.output_formats) {
            body[spec.format_field] = json!(fmt);
        }
        if let (Some(seed), Some(max)) = (base.seed, spec.seed_max) {
            if seed < 0 || seed > max {
                return Err(ProviderError::InvalidRequest(format!("'{}' accepts a seed in 0..={max}; got {seed}", model.id)));
            }
            body["seed"] = json!(seed);
        }
        if let (false, Some(t)) = (spec.texture_field.is_empty(), extras.texture) { body[spec.texture_field] = json!(t); }

        let url = format!("{}/v1/{endpoint}", self.api_base());
        // … auth::apply + inject_trace_headers + send; map errors (submit: 429/5xx/connect retryable) …
        let task_id: String = todo!("read the vendor's task id; missing → non-retryable error with the body");

        Ok(Model3dGenerationHandle {
            provider_job_id: task_id.clone(),
            provider: "acme".into(),
            model: model.id.clone(),
            // Persisted on the row at submit; the poller rebuilds the handle from it after a restart.
            stage_context: Some(json!({ "endpoint": endpoint, "status_url": format!("{}/v1/tasks/{task_id}", self.api_base()) })),
        })
    }

    async fn poll_status(&self, handle: &Model3dGenerationHandle) -> Result<Model3dGenerationPollResult, ProviderError> {
        let ctx = handle.stage_context.clone().unwrap_or_else(|| json!({}));
        let status_url = ctx["status_url"].as_str().map(str::to_string)
            .unwrap_or_else(|| format!("{}/v1/tasks/{}", self.api_base(), handle.provider_job_id));
        // … GET with auth; parse with crate::providers::video::read_poll_json(resp, "acme") …
        let data: Value = todo!();
        let meta = HashMap::from([("provider".to_string(), json!("acme")), ("task_id".to_string(), json!(handle.provider_job_id))]);

        let failed = |error: String| Model3dGenerationPollResult {
            status: GenerationStatus::Failed, progress: 100, files: Vec::new(), error: Some(error), metadata: meta.clone(),
        };
        match data["status"].as_str() {
            Some("queued") => return Ok(Model3dGenerationPollResult { status: GenerationStatus::Pending, progress: 5, files: vec![], error: None, metadata: meta }),
            Some("running") => {
                let p = data["progress"].as_u64().map(|p| p.min(99) as u8).unwrap_or(50);
                return Ok(Model3dGenerationPollResult { status: GenerationStatus::Processing, progress: p, files: vec![], error: None, metadata: meta });
            }
            Some("failed") | Some("cancelled") | Some("banned") => return Ok(failed(format!("Acme 3D failed: {}", data["error"]))),
            Some("success") => {}
            other => return Err(ProviderError::RequestFailed {
                message: format!("Acme 3D reported an undocumented status {other:?}"),
                status_code: None, provider_error: Some(data), retryable: true,
            }),
        }

        // Every mesh container the vendor produced, downloaded NOW (URLs expire).
        let mut files = Vec::new();
        if let Some(urls) = data["output"]["model_urls"].as_object() {
            for (fmt, url) in urls {
                let Some(url) = url.as_str() else { continue };
                // The mesh is the deliverable. "Try again" is an Err; "it will
                // never download" (expired, refused) is the vendor's result being
                // unavailable, which is a verdict: Ok(Failed), which fires the webhook.
                let (bytes, ct) = match self.fetch_asset(url).await {
                    Ok(v) => v,
                    Err(AssetError::Transient(e)) => return Err(e),
                    Err(AssetError::Gone(why)) => return Ok(failed(format!("Acme 3D mesh unavailable: {why}"))),
                };
                files.push(Model3dFile {
                    kind: Model3dAssetKind::Mesh,
                    format: fmt.to_ascii_lowercase(),                    // or format_of(url, &ct)
                    content_type: ct,
                    bytes,
                    polycount: data["output"]["face_count"].as_u64().map(|n| n as u32), // only if the VENDOR reports it
                    width: None,
                    height: None,
                });
            }
        }
        if !files.iter().any(|f| f.kind == Model3dAssetKind::Mesh) {
            return Ok(failed("Acme 3D reported success without a mesh".into()));
        }
        if let Some(url) = data["output"]["thumbnail_url"].as_str() {
            match self.fetch_asset(url).await {                          // a bonus: never fails a good mesh
                Ok((bytes, ct)) => files.push(Model3dFile { kind: Model3dAssetKind::Preview, format: format_of(url, &ct), content_type: ct, bytes, polycount: None, width: None, height: None }),
                Err(_) => tracing::warn!("acme 3d: skipping a preview that would not download"),
            }
        }
        Ok(Model3dGenerationPollResult { status: GenerationStatus::Completed, progress: 100, files, error: None, metadata: meta })
    }

    async fn estimate_cost(&self, model: &ModelSchema, _r: &Model3dGenerationRequest) -> Result<CostEstimate, ProviderError> {
        // Flat, like billing. Multipliers live in the catalog's variable_pricing, unapplied.
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult { healthy: self.is_configured(), message: format!("acme 3d configured: {}", self.is_configured()), latency_ms: None }
    }
}
```

`fal.rs` already does the `todo!()` bodies well, so read its versions
first: `fetch_asset` (with `AssetError`), `format_of`, the error-envelope
parser, and `trusted_queue_url` for status and result URLs the vendor
returns. Lift what fits, keeping the `AssetError` split.

`model3d/fal.rs` was being rewritten on 2026-10-12. Re-read it rather than
trusting this summary.

## Tests

Use the helper shapes from `.claude/skills/image-provider/references/adapter-template.md`,
then add the 3D cases:

```rust
fn make_extras(formats: &[&str]) -> Model3dExtras {
    Model3dExtras {
        output_formats: formats.iter().map(|s| s.to_string()).collect(),
        texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None, extra: None,
    }
}

#[test]
fn every_catalogued_acme_3d_model_resolves_to_an_endpoint() {
    let reg = registry();
    let rows: Vec<_> = reg.for_provider("acme").filter(|m| m.media_type == MediaType::Model3d).collect();
    assert!(!rows.is_empty());
    for m in rows {
        let spec = AcmeModel3dProvider::resolve_spec(&m.id);
        assert!(!spec.text_to_3d.is_empty() || !spec.image_to_3d.is_empty(), "{} resolves to no endpoint", m.id);
        // The YAML must not promise a mode the table lacks.
        assert_eq!(m.capabilities.text_to_3d, !spec.text_to_3d.is_empty(), "{} text_to_3d", m.id);
        assert_eq!(m.capabilities.image_to_3d, !spec.image_to_3d.is_empty(), "{} image_to_3d", m.id);
    }
}

#[test]
fn a_format_only_the_vendor_can_make_is_asked_for_and_derivable_ones_source_from_glb() {
    let spec = AcmeModel3dProvider::resolve_spec("acme/mesh-2");
    assert_eq!(AcmeModel3dProvider::vendor_format(&spec, &["fbx".into()]), Some("fbx".into()));
    assert_eq!(AcmeModel3dProvider::vendor_format(&spec, &["obj".into(), "stl".into()]), Some("glb".into()));
}

#[tokio::test]
async fn completion_downloads_every_mesh_and_tags_its_format() {
    // Serve a tiny but VALID glb (copy one from providers/model3d/glb.rs's writer
    // or the dashboard fixtures) at a URL whose extension is .glb, with an
    // application/octet-stream content type, and assert format == "glb".
}

#[tokio::test]
async fn success_without_a_mesh_is_failed_with_a_reason() { /* … */ }

#[tokio::test]
async fn a_preview_that_404s_does_not_fail_a_good_mesh() { /* … */ }

#[tokio::test]
async fn an_oversized_asset_is_refused_before_buffering() { /* Content-Length > MAX_ASSET_BYTES */ }

#[tokio::test]
async fn a_text_only_request_to_an_image_only_row_never_reaches_the_vendor() { /* zero received requests */ }

#[tokio::test]
async fn a_handle_rebuilt_from_the_row_polls_the_stored_status_url() {
    // Build Model3dGenerationHandle { provider_job_id, provider, model, stage_context } by hand,
    // the way poller.rs does from gen.metadata["stage_context"].
}
```

## Catalog row

```yaml
  # ─── 3D (mesh) ────────────────────────────────────────────────────────────
  # Every field below was read from the endpoint's own schema
  # (https://docs.acme.example/3d/openapi.json, 2026-10-12).
  # `output_formats` declares only what ACME emits natively. litegen derives
  # the other containers locally from a readable vendor file, so the API
  # advertises a wider set (model3d_convert::derivable_formats()).
  - id: acme/mesh-2
    provider: acme
    media_type: model3d
    display_name: Acme Mesh 2
    description: Acme Mesh 2 text- or image-to-3D, with fbx and usdz straight from the vendor.
    docs_url: https://docs.acme.example/3d/models/mesh-2
    # "20 credits ($0.20) per generation"; a textured mesh is "+10 credits" (×1.5).
    # @see https://acme.example/pricing.md (2026-10-12)
    pricing:
      base_cost_usd: 0.20
      variable_pricing: { textured_multiplier: 1.5 }
    capabilities: { text_to_3d: true, image_to_3d: true }
    prompt: { required: false, max_length: 1024 }
    params:
      output_formats:
        kind: string_array
        # Mesh2Input.output_format is a SCALAR → one vendor job → max_items 1.
        enum_values: [glb, fbx, usdz]
        max_items: 1
        default: [glb]
        label: Output formats
      texture:
        kind: bool
        default: true
        label: Textured mesh
        description: Sends `texture`. Acme charges 1.5× for a textured mesh.
      # Mesh2Input.seed: "minimum: 0, maximum: 4294967295"
      seed: { kind: seed, min: 0, max: 4294967295 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [texture_quality, face_limit]
    tags: [3d, mesh]
```

## Live structural checks

After a live run, download every asset and check it. The fal runs used
checks like these:

```rust
fn assert_valid_glb(b: &[u8]) {
    assert!(b.len() >= 20, "glb too short");
    assert_eq!(&b[0..4], b"glTF", "glb magic");
    assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 2, "glTF version");
    assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, b.len(), "declared length == actual length");
}
fn assert_valid_binary_stl(b: &[u8]) {
    assert!(!b.starts_with(b"solid"), "binary STL must not look like ASCII");
    let n = u32::from_le_bytes(b[80..84].try_into().unwrap()) as usize;
    assert_eq!(b.len(), 84 + 50 * n, "stl size is 84 + 50n");
}
fn assert_fbx(b: &[u8]) { assert!(b.starts_with(b"Kaydara FBX Binary"), "genuine FBX header"); }
```

Also check that the obj has no `mtllib` or `usemtl` lines, that the ply
header says `binary_little_endian` with counts matching the data, and that
triangle and vertex counts agree across every derived format. Record the
date, cost and results in the 3D section of the param-mapping matrix.
