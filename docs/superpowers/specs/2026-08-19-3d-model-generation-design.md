# 3D Model Generation — Design

**Status:** phase 1 implemented 2026-08-20 → 2026-09-11 as foundation + mock provider (plan: [2026-08-20-3d-model-generation](../plans/2026-08-20-3d-model-generation.md)). Vendor adapters (Meshy, Tripo3D, Stability, Rodin) open. The wire contract differs from §3, §6 and §8 — read **§14 Reconciliation** before relying on any wire-facing detail below.
**Date:** 2026-08-19
**Scope:** litegen-core, models/, dashboard, sdks/typescript, apps/landing (generated config)
**Builds on:** [Provider Expansion + Flexible Authentication](2026-05-30-provider-expansion-design.md), [BYO per-app S3 storage](2026-06-04-byo-app-storage-design.md)

---

## 1. Problem & goal

litegen unifies image and video generation across many first-party vendors behind one API, one dashboard, and one quota/billing model. This spec extends that unification to a third modality: **3D model (mesh) generation** — text-to-3D and image-to-3D — via first-party vendor APIs (Meshy, Tripo3D, Stability, Rodin/Hyper3D).

3D generation is architecturally closest to **video**: every vendor is async (submit a job, poll for completion), pricing is flat-per-generation, and quota reserve/settle is modality-agnostic already. It differs from video in one deliberate way: outputs get **re-hosted on litegen's own storage** (mirroring the **image** path, not video's provider-hosted-URL pattern), so that an app's BYO S3 credentials apply uniformly across every modality — the goal is that plugging in your own bucket "just works" the same way for a mesh as it does for an image, with no new bundle/manifest format.

### Design decisions (locked with the user)

1. **First-party integration** for Meshy, Tripo3D, Stability, and Rodin (Hyper3D). No aggregator routing.
2. **Async job/poll pattern**, mirroring `VideoProvider` — not synchronous like images.
3. **Re-host on litegen storage** (S3-compatible, including per-app BYO buckets), not provider-hosted URLs. No multi-file manifest/bundle format — each output file (mesh + optional texture maps) is stored under a predictable key, referenced by plain URLs in the response/metadata.
4. **`<model-viewer>` web component** for dashboard previews (Playground result tile + Generations gallery + trace drill-down).
5. **Full slice in phase 1**: API + one representative provider pair (Meshy, Tripo3D — chosen because their request/response shapes differ meaningfully, which proves the abstraction) + DB/enum plumbing + Playground + Generations gallery + SDK poll helper. Stability and Rodin are a phase-2 fast-follow once the trait is proven.
6. **New capability params ride the existing `extra_allowlist` escape hatch** for phase 1 (no new `ParamSpec` variants) — 3D-specific controls (topology, target_polycount, texture_prompt, art_style, etc.) are passed through as opaque extras rather than gaining first-class UI param types. A follow-up can promote the highest-value ones to `KNOWN_PARAMS` once real usage shows which matter.

### Non-goals (this spec)

- Rigging/animation, auto-retopology, or any post-generation mesh-editing endpoints these vendors also expose (e.g. Meshy's Auto-Rigging/Animation credits) — generation only.
- A dedicated multi-file bundle/manifest/zip format — each file is stored and referenced individually.
- New first-class `ParamSpec` variants for 3D-specific controls (polycount sliders, topology pickers) — phase 1 uses `extra_allowlist` passthrough; the Playground already renders unknown/extra params generically enough to not block on this, and dedicated widgets can follow once real usage data exists.
- price-api's third `MediaType` copy (`apps/price-api/src/providers/types.ts:26`) — cosmetic, deferred.
- Per-vendor rigging/texture-only endpoints as separate litegen endpoints.

---

## 2. Architecture overview

Each new provider is an independent unit, following the exact shape of an existing video provider:

1. A Rust impl in `src/providers/model3d/<vendor>.rs` implementing a new `Model3dProvider` trait.
2. A declarative `models/<vendor>.yaml` (or an addition to an existing vendor's yaml, for Stability which already has an image entry).
3. Wiring: `providers/model3d/mod.rs` (`pub mod`), `proxy/registry.rs` (`build_model3d_provider` match arm + `MODEL3D_PROVIDERS` catalog), env-var map, `litegen.example.yaml`.

The **new shared machinery** (phase 1) is:

- `providers/mod.rs`: `Model3dProvider` trait, `Model3dExtras`, `Model3dGenerationHandle`, `Model3dGenerationPollResult` — structurally identical to the video equivalents.
- `capabilities/schema.rs`: `MediaType::Model3d`, `ModelCapabilityFlags::{text_to_3d, image_to_3d}`.
- `types/mod.rs`: `MediaType::Model3d` (the `ModelInfo`-facing copy), `Model3dGenerationResponse`.
- `api/handlers/mod.rs`: `generate_3d`, `estimate_3d_cost`, `get_3d_status` — mirroring `generate_video`/`estimate_video_cost`/`get_video_status`.
- `proxy/router.rs`: `model3d_jobs: HashMap<String, Model3dGenerationHandle>` (mirrors `video_jobs`), `generate_model3d`/`get_model3d_status` router methods.
- `proxy/poller.rs`: generalized to dispatch on `media_type` between `VideoProvider` and `Model3dProvider` polling.
- `proxy/storage.rs`: a `build_model3d_store(config) -> Arc<dyn ImageStorage>` mirroring `build_image_store`, but returning the **existing, already key-explicit** `ImageStorage` trait (`put(key, bytes, content_type)`), not the extension-guessing `ImageStore` trait — see §5.
- `api/handlers/mod.rs`: `resolve_app_model3d_store()` mirroring `resolve_app_image_store()`, reading the **same** `app_storage_credentials` row (no new DB table).
- Dashboard: a `<model-viewer>`-based 3D preview component, wired into `Generations.tsx`, `Models.tsx`, `ModelDetail.tsx`, `TracePanel.tsx`, and a new async-aware Playground result-tile path (see §7 — Playground currently has **no polling machinery at all**, since image generation is synchronous; this is new, not a copy-paste).
- SDK: `sdks/typescript/src/polling.ts`'s `pollVideo`/`waitForCompletion` generalized to a generic `pollJob<T extends { status: string }>` so a `poll3d` helper is a two-line wrapper, not a duplicated file.

---

## 3. Data model

### 3.1 `MediaType` — four independent copies, all closed enums

| Location | Current | Change |
|---|---|---|
| `litegen-core/src/capabilities/schema.rs:9-12` | `Image \| Video` | + `Model3d` |
| `litegen-core/src/types/mod.rs:217-220` | `Image \| Video` | + `Model3d` |
| `apps/landing/src/config/models.generated.ts:24` | `'image' \| 'video'` | + `'model_3d'` |
| `apps/landing/src/components/infra-flow/capabilities.generated.ts:9` | `{ image: bool; video: bool }` | + `model_3d: bool` |

Both Rust enums are `#[serde(rename_all = "snake_case")]`, so `Model3d` serializes as `"model_3d"` automatically — the wire value used everywhere below (`generations.media_type`, dashboard filters, generated TS) is `"model_3d"`. The two Rust copies are genuinely independent types today (`capabilities::schema::MediaType` describes a model's declared modality; `types::MediaType` is the `ModelInfo`/OpenAPI-facing copy) — both need the variant; the compiler's exhaustive-match errors are the checklist for every call site that needs updating, exactly as the provider-expansion spec used SigV4/TC3 additions to find every touchpoint.

`generations.media_type` (`litegen-core/migrations/postgres/20240101000003_generations.sql:1-21`) is a free-text `TEXT` column, **not** a DB enum or CHECK constraint — storing `"model_3d"` there requires **zero migration**.

`ModelCapabilityFlags` (`capabilities/schema.rs:15-21`) gains `text_to_3d: bool` and `image_to_3d: bool`, following the existing `text_to_video`/`image_to_video` pattern exactly.

`GenerationStatus` (`types/mod.rs:149-155`, `Pending|Processing|Completed|Failed|Cancelled`) is reused **unchanged**.

### 3.2 `Model3dGenerationResponse`

New type in `types/mod.rs`, structurally identical to `VideoGenerationResponse` (`types/mod.rs:120-143`) with the field renamed:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dGenerationResponse {
    pub id: String,
    pub status: GenerationStatus,
    pub model: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_url: Option<String>,       // primary mesh file (GLB), litegen-hosted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,   // rendered preview image, litegen-hosted
    pub progress: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    pub created: i64,
}
```

`thumbnail_url` has no video analog but every phase-1 vendor returns one (Meshy: `thumbnail_url`; Tripo3D: rendered preview) — worth surfacing directly rather than burying it in `metadata`, since the dashboard gallery thumb view wants a 2D image even when the full row renders `<model-viewer>`. Extra per-vendor files (separate PBR texture maps, alternate export formats) go in `metadata: HashMap<String, serde_json::Value>` (already exists on `Generation`, `types/mod.rs:568-594`) as a flat `{ "texture_urls": {...}, "formats": {"fbx": "...", "usdz": "..."} }` — not a new manifest type.

### 3.3 `Model3dExtras`

Mirrors `ImageExtras`/`VideoExtras` (`providers/mod.rs:236-258`) — the handful of params common enough across vendors to deserve first-class fields; everything vendor-specific rides `extra`:

```rust
#[derive(serde::Serialize)]
pub struct Model3dExtras {
    pub art_style: Option<String>,        // Meshy: art_style
    pub topology: Option<String>,         // Meshy/Tripo: quad|triangle
    pub target_polycount: Option<u32>,
    pub should_texture: Option<bool>,
    pub texture_prompt: Option<String>,
    pub symmetry_mode: Option<String>,
    pub extra: Option<serde_json::Value>, // vendor-specific passthrough (extra_allowlist)
}
```

These field names are **not** added to `capabilities::schema::KNOWN_PARAMS` (`schema.rs:187-200`, the loader's allowlist for first-class YAML params) in phase 1 — they ride `extra_allowlist` (`schema.rs:177`) instead, so a model YAML lists them as opaque pass-through keys the validator won't reject, without the Playground needing bespoke widgets for each one yet. If real usage shows a param deserves a proper `ParamSpec` (e.g. `topology` as an enum picker), promoting it later is additive.

---

## 4. Provider abstraction

### 4.1 `Model3dProvider` trait (`litegen-core/src/providers/mod.rs`, new, alongside `VideoProvider` at line 298)

```rust
#[async_trait]
pub trait Model3dProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError>;

    async fn poll_status(&self, handle: &Model3dGenerationHandle) -> Result<Model3dGenerationPollResult, ProviderError>;

    async fn estimate_cost(&self, model: &ModelSchema, request: &Model3dGenerationRequest) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;
}

#[derive(Debug, Clone)]
pub struct Model3dGenerationHandle {
    pub provider_job_id: String,
    pub provider: String,
    pub model: String,
    /// Meshy's text-to-3d is a two-stage preview→refine pipeline; the refine
    /// call needs the preview task's id. Set by `generate()` when a provider's
    /// flow has an intermediate stage; opaque to everything except that provider.
    pub stage_context: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Model3dGenerationPollResult {
    pub status: GenerationStatus,
    pub progress: u8,
    pub model_bytes: Option<Vec<u8>>,       // primary mesh file bytes, for litegen to re-host (see §5)
    pub model_content_type: Option<String>, // e.g. "model/gltf-binary"
    pub thumbnail_bytes: Option<Vec<u8>>,
    pub thumbnail_content_type: Option<String>,
    pub extra_files: Vec<(String, Vec<u8>, String)>, // (logical name, bytes, content_type) — texture maps etc.
    pub error: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}
```

**Deliberate divergence from `VideoProvider`:** `VideoGenerationPollResult` carries a `video_url` (provider-hosted; litegen never downloads it — a `video_data` field exists but is dead code, per §5.1). `Model3dGenerationPollResult` instead carries **bytes** (`model_bytes`/`thumbnail_bytes`/`extra_files`), because per the storage decision (§5) litegen re-hosts every 3D output. This means each provider's `poll_status` must download the mesh (and thumbnail/textures) from the vendor's URL before returning — an HTTP GET inside the provider impl, same shape as `providers/video/openai.rs:323-375`'s existing (currently-unused) byte-download pattern, which becomes the template instead of dead code.

**Two-stage providers (Meshy text-to-3D only):** `generate()` for the `preview` stage returns a handle with `stage_context: Some({"preview_task_id": "..."})`; the poller sees `Completed` on the preview stage, and the provider's own `poll_status` implementation — not the generic poller — chains the `refine` call using that context and returns `Processing` until the refine task also completes. This keeps the two-stage vendor quirk entirely inside `providers/model3d/meshy.rs`; the poller loop, DB row, and API contract stay single-stage from the caller's perspective (one `id`, one `Pending→Processing→Completed` lifecycle). Tripo3D is single-stage when targeting `v2.5+`/`v3.x` (its `refine_model` chaining is legacy, pre-`v2.0` models only — confirmed in §9) and needs no `stage_context`.

**Sync-only providers (Stability, phase 2):** the inverse problem — Stability's 3D endpoints are a single synchronous POST that returns GLB bytes directly, no task/poll at all, but the trait contract still requires `generate()` to return a handle. The adapter calls the sync endpoint inside `generate()`, stashes the bytes in a short-lived in-process store keyed by a generated job id, and returns a handle whose first `poll_status()` call returns `Completed` immediately with those bytes — the mirror image of `providers/image/replicate.rs`'s existing "async vendor wrapped in a sync trait" adapter, so this has direct precedent in the codebase (details in §9).

### 4.2 Registry wiring (`litegen-core/src/proxy/registry.rs`)

- New `model3d_providers: RwLock<HashMap<String, Arc<dyn Model3dProvider>>>` field on `ProviderRegistry` (line ~47), alongside `image_providers`/`video_providers`.
- `register_provider()` (`registry.rs:99-134`) gains a third `build_model3d_provider(name, &config)` call, following the identical `Option` → insert-if-some pattern already used for image/video.
- New `build_model3d_provider()` factory (mirrors `build_image_provider`/`build_video_provider`, `registry.rs:311-379`) — hardcoded `match name { "meshy" => ..., "tripo3d" => ..., ... }`, phase 1 has two arms.
- New `MODEL3D_PROVIDERS` const slice (mirrors `IMAGE_PROVIDERS`/`VIDEO_PROVIDERS`, `registry.rs:384-395`), asserted in sync with the match arms by the existing test pattern (`registry.rs:725-746`).
- `model3d_provider_for_request()` (mirrors `image_provider_for_request`/`video_provider_for_request`, `registry.rs:144-...`) for per-app BYO **provider-API** credential override (unrelated to storage credentials — this is Meshy/Tripo3D API keys, resolved the same way OpenAI/Stability keys are today).
- `provider_catalog()` (`registry.rs:402-471`) — Meshy and Tripo3D both use a plain Bearer API key (`AuthSpec::Header`, per the provider-expansion spec's auth abstraction), so no new `AuthSpec` variant is needed; they slot into the existing catalog entries with zero auth-layer changes.

---

## 5. Storage

### 5.1 Why 3D breaks from video's pattern

Today: images are downloaded by litegen and re-uploaded to litegen's own bucket (`router.rs:914-951`); video URLs are returned as-is from the provider and never touched by `image_store` (confirmed: zero references to `image_store` in the video code path). The user's requirement — BYO S3 credentials should "just work" for 3D the same way they do for images — means 3D must follow the **image** pattern, not video's.

### 5.2 Reuse the existing `ImageStorage` trait, not `ImageStore`

There are two storage traits in `litegen-core/src/proxy/storage.rs`, and they matter differently here:

- `ImageStore::store(data, content_type, generation_id) -> URL` (`storage.rs:6-17`, impl'd by `S3Store`/`LocalStore`) — used only by the **image** generation path. Its `S3Store` impl **infers the file extension from a hardcoded content-type switch** (`storage.rs:104-109`: `image/png|jpeg|webp`, else defaults to `"png"`). Reusing this as-is for a `.glb` file would silently save it with a `.png` extension — a real bug, not a hypothetical.
- `ImageStorage::put(key, bytes, content_type) -> URL` / `delete(key)` (`storage.rs:152-157`, impl'd by `S3Storage`/`LocalStorage`) — already used by the materializer for temp ref-image uploads. This trait takes an **explicit key**, so the caller controls the extension — no content-type-sniffing bug possible. This is the correct reuse point for 3D.

**Design:** a `build_model3d_store(config: &ImageStorageConfig) -> Arc<dyn ImageStorage>` function (mirrors `build_image_store`, `storage.rs:232-250`, but returns `S3Storage`/`LocalStorage` instead of `S3Store`/`LocalStore`), and a corresponding `resolve_app_model3d_store()` handler function (mirrors `resolve_app_image_store`, `handlers/mod.rs:2623-2673`) that reads the **same** `app_storage_credentials` DB row — no new table, no new encryption, no new dashboard form. An app that has already configured BYO storage for images gets it for 3D outputs automatically, for free.

Each file for a generation is stored with an explicit, predictable key — no manifest object, just a flat prefix:

```
{path_prefix}/{generation_id}/model.glb
{path_prefix}/{generation_id}/thumbnail.png
{path_prefix}/{generation_id}/texture_base_color.png   (only if the vendor returns separate maps)
```

`path_prefix` defaults to `"litegen/3d"` (distinct from images' `"litegen/images"`, same `ImageStorageConfig.path_prefix` field, just a different default constant for this call site — no schema change).

### 5.3 Response shape

`Model3dGenerationResponse.model_url`/`.thumbnail_url` hold the litegen-hosted URLs for the primary GLB and thumbnail (§3.2); any additional files (extra export formats, separate PBR maps) are listed in `Generation.metadata` as a flat URL map — consistent with how video's provider-specific extras already ride `metadata` today, not a new pattern.

---

## 6. Async job & polling infra

### 6.1 API surface

New routes in `litegen-core/src/api/handlers/mod.rs`, registered alongside the existing table (`handlers/mod.rs:2013-2018`):

```
POST /v1/3d/generations   → generate_3d          (ASYNC job — mirrors generate_video, handlers/mod.rs:345-470)
POST /v1/3d/cost          → estimate_3d_cost      (mirrors estimate_video_cost)
GET  /v1/3d/{id}          → get_3d_status         (mirrors get_video_status, handlers/mod.rs:484-517)
```

`GET /v1/generations/{id}` (the DB-backed, cross-modal poll endpoint) and `GET /v1/generations` (list) work for 3D **with zero changes** — both already read `media_type` as an opaque string.

`generate_3d` follows `generate_video`'s exact structure (`handlers/mod.rs:345-470`): materialize refs → build `Model3dExtras` → resolve BYO **provider** credential (`resolve_org_provider_credential`) → resolve BYO **storage** (`resolve_app_model3d_store`, new — passed to the router the same way `app_store` is passed to `generate_image` at `handlers/mod.rs:168,186`, which `generate_video` today does *not* do) → `reserve_quota` → dispatch → on success, `insert_generation(..., "model_3d", ...)` (the existing `DatabaseStore::insert_generation` signature already takes `media_type: &str`, no interface change) → `settle_quota`.

### 6.2 Router (`litegen-core/src/proxy/router.rs`)

- `model3d_jobs: Arc<RwLock<HashMap<String, Model3dGenerationHandle>>>` field (mirrors `video_jobs`, line 29).
- `generate_model3d(...)` router method: dispatches to the provider, and on receiving a `Model3dGenerationPollResult` with `Completed` status inline (some vendors may resolve fast enough to complete within the initial call, though most won't), immediately persists bytes via the resolved `ImageStorage` — otherwise the poller does this on a later tick (§6.3).
- `get_model3d_status(id)`: mirrors `get_video_status` — checks `model3d_jobs` first (immediate post-submission poll), falls through to the DB row.

### 6.3 Poller (`litegen-core/src/proxy/poller.rs`)

`poll_once()` (`poller.rs:35-218`) currently resolves every active row through `registry.video_provider_for_request(...)` unconditionally (`poller.rs:67-70`) — it has no modality branch because only video is async today. Generalize:

```rust
match gen.media_type.as_str() {
    "video" => { /* existing VideoProvider path, unchanged */ }
    "model_3d" => {
        let provider = registry.model3d_provider_for_request(&gen.provider, app_creds).await;
        // poll_status → on Completed, upload model_bytes/thumbnail_bytes/extra_files via
        // the resolved ImageStorage (per-app if configured, else the global model3d store),
        // then update_generation_status with the litegen-hosted result_url.
    }
    _ => { /* unreachable for pending/processing rows today; log + skip */ }
}
```

The reaper (`MAX_ACTIVE_AGE_SECS`, `poller.rs:17`), webhook dispatch on terminal transition (`poller.rs:158-216`), and the credential-resolution/error-swallowing discipline (`poller.rs:61-65`) are unchanged and apply identically to 3D rows — none of that logic is video-specific in *behavior*, only in the one `video_provider_for_request` call site.

**Download-and-rehost must happen within the same poll tick that observes completion, not lazily.** Tripo3D's docs are explicit that model URLs expire 5 minutes after task success (§9); Rodin's separate download-URL step has the same property. Video's poller never had this pressure (provider URLs are long-lived, since litegen never re-hosts them) — for 3D, `poll_status()` returning `Completed` must come back with bytes already in hand (the provider impl downloads internally), so the poller's upload-to-`ImageStorage` step happens immediately in the same iteration, before the next 5-second poll cycle even starts.

---

## 7. Dashboard

### 7.1 Generations gallery (`dashboard/src/pages/Generations.tsx`) — closest existing pattern, extend directly

`MediaPreview` (`Generations.tsx:19-43`) currently does `isVideo = g.media_type === 'video'` and picks `<video>` vs `<img>`. Becomes a 3-way switch:

```tsx
if (g.media_type === 'model_3d') {
  return thumb
    ? <img src={g.metadata?.thumbnail_url ?? g.result_url} .../>   // thumb mode: still a flat 2D image
    : <model-viewer src={g.result_url} camera-controls auto-rotate style={style} />;
}
```

`<model-viewer>` ships as a single `<script type="module" src=".../model-viewer.min.js">` tag (no bundler-level three.js dependency); add it once to `dashboard/index.html` or lazy-import it in the one component that uses it.

`Models.tsx:88`'s hardcoded `['', 'image', 'video']` filter dropdown, `ModelDetail.tsx:7-8`'s `isVideo ? '/v1/videos/generations' : '/v1/images/generations'` curl-builder, and `TracePanel.tsx:220-227`'s artifact drill-down all get the same 3-way extension — mechanical, no new patterns.

### 7.2 Playground — genuinely new work, not a copy-paste

Two things make this the least "just wire it up" part of the plan:

1. **The Playground is mock-only today, by design.** `SingleMode.tsx:50-51` filters models to `m.media_type === 'image' && m.provider === 'mock'` — it's a free, deterministic sandbox, not a "generate with any configured provider" surface. So a 3D Playground entry only needs a **mock 3D provider** registered (§8.2), not real Meshy/Tripo3D credentials — consistent with how the Playground already works for images and keeps it usable in any dev/CI environment.
2. **The Playground has no async/polling state machine at all**, because image generation is synchronous. `ResultTileState` (`playground/types.ts:24-35`) has exactly two non-terminal statuses (`queued`, `running` — both client-side, covering "not yet dispatched" and "request in flight") and expects `b64_json`/`url` back from the *same* request. 3D (and, incidentally, video, which isn't in the Playground either) needs a genuine `pending`/`processing` → poll → `done` path. This is new state-machine work, scoped to the Playground's request-dispatch hook (`useUnifiedParams.ts` / wherever `ResultTileState` transitions are driven) — reuse the SDK's generalized `pollJob` helper (§8.3) as the polling primitive so this isn't hand-rolled twice.
3. A new `ResultTile3D` (sibling to `playground/ResultTile.tsx`) renders `<model-viewer>` instead of `<img>`, reusing the same head/meta/rerun chrome.
4. `ModelPicker.tsx`, `useModelSchemas.ts`, `ParamField.tsx` need **no changes** — they already render controls generically from `ModelSchema.params`, which is exactly why phase 1 can lean on `extra_allowlist` passthrough (§3.3) without the Playground needing bespoke 3D widgets on day one.

---

## 8. SDK & generated config

### 8.1 OpenAPI

`#[utoipa::path]` annotations on the three new routes (§6.1) → `apps/landing/public/openapi.json` regenerates via the existing utoipa pipeline → `sdks/scripts/regen-all.sh` regenerates both SDKs.

### 8.2 Mock provider (test/Playground substrate)

`litegen-core/src/providers/model3d/mock.rs`, directly modeled on `providers/video/mock.rs:1-171` (same `should_render_visual_*` allowlist pattern, same `ProviderInstanceConfig`/`configure`/`is_configured` boilerplate). Instead of a GIF, the visual mock generates (or ships as a fixture) a tiny valid GLB — a single-triangle mesh is enough; the point is a byte-valid file `<model-viewer>` can actually render, not a placeholder URL, matching how `mock/visual-image-gen` and `mock/visual-video-gen` produce real bytes today so the Playground shows genuine output. `models/mock.yaml` gains `mock/visual-3d-gen` (and a couple of param-coverage variants, following the existing `mock/all-params-image`-style convention) with `media_type: model_3d`.

### 8.3 `sdks/typescript/src/polling.ts`

Generalize rather than duplicate:

```ts
type WithStatus = { status: string };

export async function* pollJob<T extends WithStatus>(
  id: string,
  getStatus: (id: string) => Promise<T>,
  opts: WaitForCompletionOptions = {},
): AsyncGenerator<T, T, void> { /* body identical to today's pollVideo — parameterized by T */ }

export const pollVideo = (id: string, getStatus: (id: string) => Promise<VideoResponse>, opts?) =>
  pollJob<VideoResponse>(id, getStatus, opts);
export const poll3d = (id: string, getStatus: (id: string) => Promise<Model3dResponse>, opts?) =>
  pollJob<Model3dResponse>(id, getStatus, opts);
```

`waitForCompletion` gets the same generic treatment. This is a same-file refactor of `polling.ts:1-56` (types stay the same, `TERMINAL_STATUSES`/`sleep` are already type-agnostic) — not a second parallel file.

### 8.4 Landing generated config

`apps/landing/scripts/derive-models.mjs` / `derive-capabilities.mjs` (which transform `GET /v1/models` into `models.generated.ts` / `capabilities.generated.ts`) get a third boolean/literal, following the exact image→video precedent already in that code. Fail-open behavior (keep the last committed snapshot if the gateway is unreachable at build time) is preserved unchanged.

---

## 9. Provider-specific notes (phase 1)

### Meshy — provider `meshy` (new, text+image-to-3D)
- **Obtainable:** self-serve, free tier (100 credits/mo) · key at `meshy.ai/settings/api`
- **Base/auth:** `https://api.meshy.ai` · `Authorization: Bearer ${MESHY_API_KEY}`
- **Flow:** async task+poll throughout. **Text-to-3D is two-stage**: `mode: "preview"` (untextured geometry) then a separate `mode: "refine"` call referencing `preview_task_id` (adds PBR texturing) — see §4.1's `stage_context` design. **Image-to-3D is single-stage** (`should_texture` folds texturing into one task).
  - Text-to-3D: `POST /openapi/v2/text-to-3d` → `GET /openapi/v2/text-to-3d/:id`
  - Image-to-3D: `POST /openapi/v1/image-to-3d` → `GET /openapi/v1/image-to-3d/:id`
  - Multi-image (1–4 views): `POST /openapi/v1/multi-image-to-3d`
- **Models:** `ai_model: "meshy-5" | "meshy-6" | "meshy-7" | "latest"`; separate `model_type: "smart-topology"` variant.
- **Output:** `model_urls` (per-format: `glb`, `fbx`, `obj`, `mtl`, `usdz`, `stl`, `3mf`), `texture_urls` (array of `base_color`/`metallic`/`normal`/`roughness`/`emission` maps), `thumbnail_url` (+ `thumbnail_urls.{front,right,back,left}` for image-to-3d).
- **Refs:** image input is URL or base64 data URI, no multipart.
- **Notable params:** `art_style`, `symmetry_mode`, `topology` (`quad`/`triangle`), `target_polycount` (100–300,000), `texture_prompt`, `texture_resolution` (`2k`/`4k`/`8k`), `should_texture`, `should_remesh`, `pose_mode`.
- **Pricing:** credits; text-to-3D preview 20cr (+5 Ultra), refine 10–15cr; image-to-3D 20cr untextured / 30cr textured / 35cr at 8K. Free 100cr/mo, Pro $20/1000cr, Premium $40/3000cr.
- **Docs:** docs.meshy.ai/en/api/{quick-start,text-to-3d,image-to-3d,multi-image-to-3d,pricing}
- **Quotes:**
  - `curl https://api.meshy.ai/openapi/v2/text-to-3d -H 'Authorization: Bearer ${YOUR_API_KEY}' -d '{"mode": "preview", "prompt": "a monster mask"}'` — docs/text-to-3d
  - `curl https://api.meshy.ai/openapi/v1/image-to-3d -X POST -H "Authorization: Bearer ${MESHY_API_KEY}" -d '{"image_url": "..."}'` — docs/quick-start
  - `"data:image/jpeg;base64,<your base64-encoded image data>"` — docs/image-to-3d
- **Caveats:** confirm the `openapi/v2` (text) vs `openapi/v1` (image) version split against a live account before coding — one secondary source disagrees. Multi-image-to-3d shape not directly doc-fetched; pull the raw page first. API key prefix (`msy_` vs `msy-`) unconfirmed to the character.

### Tripo3D — provider `tripo3d` (new, text+image+multiview-to-3D)
- **Obtainable:** self-serve · keys at `platform.tripo3d.ai` → API Keys
- **Base/auth:** target the **V3** surface (forward-looking; V2's retirement date is unconfirmed but V3 is clearly current): `https://openapi.tripo3d.ai/v3` · `Authorization: Bearer <token>`
- **Flow:** async task+poll, and — unlike Meshy — **single-stage for the model tier this integration targets**. `refine_model`/`draft_model_task_id` chaining exists but only applies to legacy pre-`v2.0-20240919` models; `v2.5-20250123`+/`v3.x` generate final-quality output (mesh + texture) in one pass. So Tripo3D needs **no** `stage_context` handling — a simpler `Model3dProvider` impl than Meshy's.
  - Text-to-3D: `POST /generation/text-to-model` → `GET /tasks/{task_id}`
  - Image-to-3D: `POST /generation/image-to-model` → `GET /tasks/{task_id}`
  - Multiview-to-3D (4 named views, front mandatory, min 2): `POST /generation/multiview-to-model`
- **Models:** `model: "v3.1-20260211"` (latest) or `"v2.5-20250123"` (SDK default/balanced) in the request body.
- **Output:** single GLB by default (`quad:true` forces FBX); PBR maps are **baked into the model file**, not returned as separate map URLs, when `pbr:true`. Returns both a model URL and a rendered preview/thumbnail URL. **Critical operational detail: "Model URLs expire after 5 minutes. Download immediately after the task succeeds."** — direct confirmation that the re-hosting design in §5 isn't optional polish; the poller must download and re-upload within the same tick it observes `status:"success"`, not on a later pass. Field names need live confirmation (`model_url`/`rendered_image_url` per V3 quick-start vs `model`/`rendered_image` per the V2-shaped OpenAPI schema) before coding the deserializer.
- **Refs:** image input via `file_token` (pre-upload through `POST /upload`) **or** a direct public URL **or** a prior `task_id` — no base64 field.
- **Notable params:** `texture_alignment`, `orientation`, `style`, `face_limit`, `auto_size`, `quad`, `texture_quality`, `geometry_quality` (v3.0+), `model_seed`/`texture_seed`.
- **Pricing:** 100 credits = $1.00. Text-to-3D 10cr (no texture) / 20cr (textured). Image/multiview-to-3D 20cr / 30cr.
- **Docs:** developers.tripo3d.ai/en/docs/{quick-start,generation-text-to-model/standard,generation-image-to-model/standard}, platform.tripo3d.ai/docs/schema
- **Quotes:**
  - `curl -X POST https://openapi.tripo3d.ai/v3/generation/text-to-model -H "Authorization: Bearer <token>" -d '{"prompt": "a cute cat", "model": "v3.1-20260211"}'` — docs/quick-start
  - `curl -X GET https://openapi.tripo3d.ai/v3/tasks/{task_id} -H "Authorization: Bearer <token>"` — same
  - "Model URLs expire after 5 minutes. Download immediately after the task succeeds." — docs/quick-start
- **Caveats:** confirm V3 output field names against a live call before coding the deserializer (quick-start examples and the OpenAPI schema disagree); V2 retirement timeline unconfirmed but irrelevant if targeting V3 from the start.

### Stability (image-to-3D) — provider `stability` (extend existing image adapter)
- **Obtainable:** self-serve, **same account/key litegen already has configured for Stability image generation** — `platform.stability.ai/account/keys` is account-wide, not product-scoped. No new credential onboarding.
- **Base/auth:** `https://api.stability.ai` · `Authorization: Bearer sk-...` — identical scheme to the existing `providers/image/stability.rs` adapter.
- **Flow — the one genuinely different vendor in this set: fully synchronous, single-call, no task/poll at all.** `POST /v2beta/3d/stable-fast-3d` or `POST /v2beta/3d/stable-point-aware-3d` returns the GLB bytes directly in the response body. Since `Model3dProvider::generate()` returns a job handle by contract (§4.1), the Stability adapter calls the sync endpoint inside `generate()`, stashes the returned bytes in a short-lived in-process store keyed by a locally-generated job id (the same "stash bytes, hand back a handle" pattern `providers/video/mock.rs:101` already uses for its GIF store), and returns a handle whose very first `poll_status()` call immediately returns `Completed` with those bytes. This is the mirror image of `providers/image/replicate.rs`'s existing "async-underneath, sync-trait-signature" adapter — same technique, opposite direction — so it has direct precedent in this codebase, not a novel pattern.
- **Models:** two endpoints act as the model selector — `stable-fast-3d` and `stable-point-aware-3d` (SPAR3D, **Stability's own docs mark this "currently in preview"** — flag as lower-confidence-contract in the model YAML's tags). No TripoSR hosted endpoint exists (open-weight only).
- **Output:** single GLB blob per request, no separate texture files.
- **Refs:** `multipart/form-data`, field `image` (binary; NOT base64/URL like Meshy/Tripo — the materializer's existing `RefProviderFormat::Multipart` form, already used by Ideogram/Recraft, covers this with zero new materializer work).
- **Notable params:** `texture_resolution` (512/1024/2048), `foreground_ratio`, `remesh` (none/quad/triangle), `vertex_count` (fast-3d) or `target_type`/`target_count`/`guidance_scale`/`seed` (point-aware-3d).
- **Pricing:** flat credits, 1cr = $0.01. Stable Fast 3D: 10cr ($0.10)/generation. Stable Point Aware 3D: 4cr ($0.04)/generation. Failed generations not charged.
- **Docs:** platform.stability.ai/docs/api-reference, /pricing, /docs/release-notes
- **Quotes:**
  - `"https://api.stability.ai/v2beta/3d/stable-fast-3d"` — api-reference Python sample
  - "Flat rate of 10 credits per successful generation. You will not be charged for failed generations." — Stable Fast 3D docs
  - "This API is currently in preview." — Stable Point Aware 3D description
- **Caveats:** **not discontinued** (checked explicitly against the worry that Stability sunsets hosted media APIs the way it did Stable Video Diffusion — neither 3D endpoint appears in any deprecation notice, both are live in the current pricing table). SPAR3D's "preview" label means its contract could still shift. 10MiB request body cap matters for large input images; shared platform rate limit 150 req/10s.

### Rodin / Hyper3D — provider `rodin` (phase 2)
- **Obtainable:** self-serve per the docs ("create an account, request API access, generate an API key"), keys at `hyper3d.ai/workspace/api-dashboard` — the phrase "request API access" should be re-verified at implementation time for whether issuance is instant or gated, since every other phase-1/2 vendor here is confirmed frictionless.
- **Base/auth:** `https://api.hyper3d.com/api/v2` · `Authorization: Bearer ${RODIN_API_KEY}`. Multipart for the submit call, JSON for status/download.
- **Flow:** async, and uniquely **three-call** rather than two: submit → poll status (`POST /api/v2/status`, wait ≥5s before first poll, honor `Retry-After` on 429) → **separate download call** (`POST /api/v2/download` with `task_uuid`, once `Done`) that returns a list of `{name, url}` file entries whose URLs can also expire — so, like Tripo3D, the download step must happen promptly and feed straight into litegen's re-host, not get deferred.
- **Models:** selected via a `tier` string, not a model field — Gen-1/1.5 (`Sketch`/`Regular`/`Detail`/`Smooth`), `Gen-2`, or `Gen-2.5-{Extreme-Low,Low,Medium,High,Extreme-High}` (docs: "Always send a Gen-2.5 tier explicitly").
- **Output:** `format` param (`glb`/`usdz`/`fbx`/`obj`/`stl`, default `glb`); the one fetched download-response example showed a single geometry file entry, though marketing copy references multi-format workflows — **whether PBR maps ever arrive as separate list entries is unconfirmed**, budget live-key verification time before finalizing the deserializer.
- **Refs:** multipart `images` field (1–5 files; multi-image `condition_mode`: `concat`|`fuse`). No base64/URL input found in the native docs (a URL-based field exists only in a third-party fal.ai wrapper targeting an older API generation — do not conflate the two schemas).
- **Notable params:** `mesh_mode` (`Raw`|`Quad`), `quality` (polycount target, range vendor/tier-dependent), `material` (`PBR`|`Shaded`|`All`|`None`), `texture_mode`, `uhd_texture`, `addons` (e.g. `HighPack`).
- **Pricing:** credits, Gen-2.5 "from 1 credit/generation," Gen-2 "from 0.5 credit," $1.5/credit direct purchase or bundled subscriptions.
- **Docs:** docs.hyper3d.ai/en/{get-started/quick-start,api-specification/rodin-gen2-5,api-specification/download-results}
- **Quotes:**
  - `curl --request POST 'https://api.hyper3d.com/api/v2/rodin' --header "Authorization: Bearer ${RODIN_API_KEY}" --form 'images=@./input.png' --form 'tier=Gen-2.5-Medium'` — quick-start
  - "Wait at least five seconds before the first check" — quick-start
  - `{"message": "Submitted.", "uuid": "...", "jobs": {"uuids": [...], "subscription_key": "..."}, "consumed": 0.5}` — generation response shape
- **Caveats:** thinnest/least-mature docs of the four vendors here (JS-rendered site, some fields not conclusively confirmed) — this is *why* it's phase 2, after the trait abstraction is already proven against three better-documented vendors. Re-verify the "request API access" onboarding friction and the separate-texture-maps question against a live key before committing to the implementation.

---

## 10. Testing strategy

- **Unit/loader:** `models/*.yaml` round-trip for `meshy.yaml`/`tripo3d.yaml` (new models parse, `KNOWN_PARAMS`/`extra_allowlist` validated); `MODEL3D_PROVIDERS` const-vs-match-arm sync test (mirrors `registry.rs:725-746`).
- **Mock (wiremock) per provider:** outbound request shape for both the preview and refine calls (Meshy), auth header, poll→bytes-download happy path, error mapping — same harness/helpers as existing video provider tests.
- **Mock provider tests** (`providers/model3d/mock.rs`): directly modeled on `providers/video/mock.rs`'s test module (`mock.rs:173-356`) — `generate` → `poll_status` → assert valid GLB magic bytes stored, assert byte-for-byte difference between distinct prompts (parity with the video mock's "keyframe blend must differ from prompt-only fallback" assertion).
- **Storage:** unit test that `build_model3d_store` picks the extension from the explicit key (not content-type sniffing) — regression test for the exact bug identified in §5.2; integration test (extending the `wiremock`-S3 harness from the BYO-storage spec's `multitenant_api.rs` tests) that a 3D generation with per-app storage configured uploads to `{path_prefix}/{generation_id}/model.glb` at the app's bucket, mirroring that spec's test 4.
- **Poller:** unit test that `poll_once` dispatches `model_3d` rows to `Model3dProvider` and `video` rows to `VideoProvider` (regression guard against the media_type branch in §6.3 silently falling through the `_ => skip` arm).
- **API integration:** `POST /v1/3d/generations` → `GET /v1/3d/{id}` → `GET /v1/generations/{id}` happy path against the mock provider; quota reserve/settle assertions identical in shape to the existing video quota tests.
- **Dashboard/Playwright:** extend the existing Playground/Generations e2e specs with a `model_3d` row — assert `<model-viewer>` renders with the mock GLB's `src`, assert the Playground's new pending→poll→done tile transition completes.
- **Live (`#[ignore]`):** one real Meshy + one real Tripo3D round-trip, gated by `MESHY_API_KEY`/`TRIPO3D_API_KEY` env vars, following the provider-expansion spec's `tests/live/` self-skip pattern exactly.

Acceptance: `cargo test -p litegen-core` green, `cargo clippy --all-targets -- -D warnings` clean, `cd dashboard && npm run build && npx playwright test` clean, `cd sdks/typescript && npm run build && npm test` clean.

---

## 11. Phase plan

- **Phase 1 — Foundation + Meshy + Tripo3D + full dashboard slice.** Everything in §3–§8 for two providers. Ships a genuinely usable end-to-end feature (API, SDK, Playground, gallery).
- **Phase 2 — Stability + Rodin fast-follow.** New `providers/model3d/{stability,rodin}.rs` on the now-proven trait; no architectural changes expected. Rodin's onboarding friction ("request API access") and separate-texture-maps handling need re-confirming against a live key at that time (§9 caveats).
- **Phase 3 (optional, not scoped here) — promote high-value 3D params out of `extra_allowlist`** into first-class `ParamSpec` entries once real usage shows which controls (topology? polycount? texture resolution?) users actually reach for in the Playground.

---

## 12. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Re-hosting requires downloading vendor-hosted mesh+texture bytes server-side before upload — adds latency vs. video's "just pass the URL through" | Accepted tradeoff per the locked BYO-storage requirement (§5.1); poller already tolerates multi-second per-row work, and mesh files are typically smaller than video files |
| `S3Store`'s content-type→extension bug (§5.2) could get copy-pasted into a 3D path if someone reaches for `ImageStore` instead of `ImageStorage` | Named explicitly in this spec + a regression unit test (§10) asserting extension comes from the explicit key |
| Meshy's two-stage preview/refine flow leaking into the generic poller | Contained entirely inside `providers/model3d/meshy.rs` via `stage_context` (§4.1) — the poller/DB/API stay single-stage |
| Vendor API drift (Meshy's `v1`/`v2` endpoint-version split, Tripo3D's field-name ambiguity between quick-start and OpenAPI schema, Stability SPAR3D's "preview" label, Rodin's thinly-documented download response) | Pin conservative model IDs; wiremock tests lock outbound shape; `#[ignore]`d live tests catch real drift on demand; each caveat re-verified against a live key at implementation time per §9 |
| Four independent `MediaType` enums must all move together | Compiler-enforced for both Rust copies (exhaustive match); TS copies are the only place a missed spot compiles silently — covered by the Playwright 3D row test (§10) |
| `extra_allowlist` passthrough means malformed vendor-specific params fail at the provider HTTP call, not at litegen's validator | Same failure mode video/image already accept for their own `extra` fields; wiremock tests cover the well-formed shape, and `ProviderError::InvalidRequest` surfaces vendor 4xx cleanly |

---

## 13. Implementation order

1. **Data model**: `MediaType::Model3d` in both Rust enums (let the compiler find every match-arm gap), `ModelCapabilityFlags::{text_to_3d,image_to_3d}`, `Model3dGenerationResponse`/`Model3dExtras`/`Model3dGenerationHandle`/`Model3dGenerationPollResult` types.
2. **`Model3dProvider` trait** + `providers/model3d/mock.rs` (TDD substrate for everything downstream) + `models/mock.yaml` 3D entries.
3. **Storage**: `build_model3d_store`, `resolve_app_model3d_store`, extension-from-key regression test.
4. **API**: `generate_3d`/`estimate_3d_cost`/`get_3d_status` handlers + router `model3d_jobs` + route table wiring. Integration tests against the mock provider green.
5. **Poller**: media_type dispatch branch + unit test.
6. **Real providers**: Meshy (two-stage), then Tripo3D — wiremock test first (TDD), then implementation, then `#[ignore]` live test, per provider.
7. **OpenAPI → SDK regen**: `pollJob<T>` generalization + `poll3d` export.
8. **Dashboard**: `<model-viewer>` integration, `Generations.tsx`/`Models.tsx`/`ModelDetail.tsx`/`TracePanel.tsx` 3-way extensions, Playground pending/poll state machine + `ResultTile3D`.
9. **Landing config**: `derive-models.mjs`/`derive-capabilities.mjs` third-output extension, regenerate committed snapshots.
10. **Polish**: clippy/build/lint clean across all four workspaces (litegen-core, dashboard, sdks/typescript, apps/landing).

---

## 14. Reconciliation with what shipped (2026-09-11)

Implemented 2026-08-20 → 2026-09-11 by
[the plan](../plans/2026-08-20-3d-model-generation.md) (Tasks 1–22 plus
follow-ups 17A–17C) as **foundation + mock provider**. **No vendor adapter was
built:** the user scoped Meshy/Tripo3D/Stability/Rodin out of this pass (the
plan's scope question, re-confirmed at the Task 4 review) until live keys can
settle §9's field-name caveats. The trait, `MODEL3D_PROVIDERS` /
`build_model3d_provider`, and the catalog conformance suite are shaped so each
vendor lands as an additive match arm plus a `models/<vendor>.yaml`.

### 14.1 Contract reconciliation — aipix's contract won on the wire

aipix, the first consumer, was already coded against its own contract request
(`docs/litegen-3d-specs.md` on aipix's `3d-model` branch) when implementation
began. The user ruled (2026-08-20) that that document wins on **every
wire-facing decision** and this spec wins on **internal architecture** — the
consumer already existed and this design did not, and aipix's shapes were
already written in litegen's own vocabulary (`BaseGenerationRequest`,
`GenerationStatus`, `ModelSchema`, `UsageInfo`). Changes from §3, §6 and §8:

- **Wire media type is `model3d`, not `model_3d`.** §3.1's claim that `Model3d`
  serializes as `"model_3d"` is wrong — serde's `snake_case` does not insert an
  underscore before a digit. Both Rust enums, `generations.media_type`, the
  dashboard and the generated TS all carry `model3d`.
- **Routes are `/v1/models3d/…`, not `/v1/3d/…` (§6.1):**
  `POST /v1/models3d/generations`, `POST /v1/models3d/cost`,
  `GET /v1/models3d/{id}`. Cancellation is the shared
  `PATCH /v1/generations/{id}`.
- **The response carries `assets: Model3dAsset[]`**, not
  `model_url`/`thumbnail_url` with extras in `metadata` (§3.2, §5.3). Each
  asset is `{kind: mesh|texture|preview, url, format, size_bytes?, polycount?,
  width?, height?}`; a completed generation carries exactly one
  `kind: "mesh"`, and texture/preview files are siblings rather than a separate
  map. The same list persists to `Generation.metadata.assets`. The provider
  side follows suit: `Model3dGenerationPollResult` carries
  `files: Vec<Model3dFile>` (kind-tagged, already-downloaded bytes) instead of
  §4.1's `model_bytes` / `thumbnail_bytes` / `extra_files`.
- **3D params are first-class `ParamSpec` entries — reversing decision 6 and
  the §1/§3.3 non-goal.** aipix renders one control per `params` entry, so
  `extra_allowlist` passthrough would have left every 3D knob unrenderable.
  `KNOWN_PARAMS` gained `output_format`, `texture`, `pbr`,
  `target_polycount`, `symmetry`, `topology`, `rig`; `Model3dExtras` carries
  exactly those (not §3.3's `art_style` / `should_texture` /
  `texture_prompt` / `symmetry_mode`). Every `ParamSpec` variant also gained
  optional `label` / `description` (aipix's "one request beyond 3D"), which
  the Playground's `ParamField` now renders — so §7.2 item 4's "no changes"
  did not hold.
- **`strict: false` drops unsupported params and reports them in
  `X-Litegen-Dropped-Params`** instead of erroring, as for images
  (`validate_model3d` + the `ValidatedModel3d` extractor) — the behaviour
  aipix calls the most important in its contract.
- **Capabilities:** `ModelCapabilityFlags` gained `multiview_to_3d` beside
  `text_to_3d` / `image_to_3d`; mode is inferred from reference roles (none →
  text, `init` → image, `view-*` → multiview). `ModelCapabilities` (the
  `GET /v1/models` projection) gained `supports_{text,image,multiview}_to_3d`,
  `supports_{pbr,rig,texture}`, `output_formats`, `max_polycount`.
- **Completed without a mesh is a failure.** Both paths that can observe
  completion — `get_3d_status` and the poller — terminalise such a row as
  `failed` with an error (the poller through its normal terminal tail, so the
  webhook fires). aipix refunds the slot on that state; reporting it as
  `completed` would bill a user for nothing.
- **Every asset URL is absolute.** Keys are
  `{path_prefix}/{generation_id}/{model|preview|texture}[_n].{ext}` under
  `litegen/3d` (as §5.2). New: with no S3 configured, a local fallback keeps
  the bytes in process memory and serves them unauthenticated at
  `GET /v1/models3d/assets/{*key}`, with URLs minted from the new
  `server.public_base_url` setting (default `http://{host}:{port}`). §5 had no
  local path.
- **SDK surface is aipix §5's:** `client.models3d.{generate, estimateCost,
  getStatus, waitForCompletion, poll}` and `Model3dJob` (awaitable +
  async-iterable, like `VideoJob`), on top of §8.3's `pollJob<T>` / `poll3d`.
  The Python SDK regenerated its types only.
- **Mock provider:** `mock/mesh-3d` (the id aipix already seeded),
  `mock/all-params-3d` (every 3D param + multiview roles), and `mock/fail-3d`
  — not §8.2's `mock/visual-3d-gen`. They serve a real prompt-tinted glTF 2.0
  binary cube written by `providers/model3d/glb.rs`, with progress 33 → 66 →
  100 across three polls.

Everything else shipped as designed: the `Model3dProvider` trait shape
(including `stage_context` for two-stage vendors), byte-carrying poll results,
`ImageStorage` not `ImageStore` (§5.2), per-app BYO storage through the same
`app_storage_credentials` row, same-tick re-hosting, the poller's media-type
branch, `pollJob<T>`, and the landing generators' third output (§8.4). One
internal deviation: `ProxyRouter::new` keeps its signature and derives the 3D
store from the config it already holds (34 call sites, 33 of them test
harnesses); `poll_once` / `spawn_poller` take the store, and the router and
poller share one instance — required, because the local store is
process-global.

### 14.2 Shipped beyond this spec

- **`ModelPreview` inspector (Task 21):** auto-rotate, camera reset, exposure,
  background, fullscreen, copy-URL, a stats strip (format, size, polycount,
  bounding box), asset list and thumbnail strip. `ModelViewer` observes
  `<model-viewer>`'s own `load` / `error` / `progress` events and never shows
  an empty canvas: bundle-import failure, no WebGL2, a non-glTF format and a
  failed load each get an explicit fallback, and Retry busts the loader cache
  with a `#retry-N` fragment. `<model-viewer>` is an npm dependency
  lazy-imported by one component, not a script tag (§7.1). No wireframe mode —
  `<model-viewer>` has no API for one.
- **Observability resolution (Task 22):** the Logs trace panel resolves an
  async request's artifact to its generation row (`client.generations.get`,
  new in the SDK) and renders it through `GenerationOutput`, rather than adding
  a DB artifact-update path that would be a second source of truth. Fixed the
  identical video bug for free.
- **Storybook (Task 20):** Storybook 10.6 (`@storybook/react-vite`) stories for
  ModelViewer, ModelPreview, GenerationOutput, MediaPreview, ResultTile3D,
  ResultTile, ResultGrid and ParamField, rendering real GLB/PNG fixtures from a
  committed stdlib `make_fixtures.py`; every story was driven in headless
  Chromium. The dashboard also gained vitest for its pure helpers.
- **Param-mapping matrix (Task 18):** `docs/superpowers/param-mapping-matrix.md`,
  a tickable request-flow/param-mapping checklist for every family, where ✅
  means a named test proves the mapping.
- **Catalog conformance (Task 19):** `tests/catalog_conformance.rs` asserts the
  model3d family's structural contract — mode vs. ref-role coherence, enum
  vocabularies, no image/video-only params, sane polycount bounds.
- **Playwright (Task 15):** `dashboard/e2e/model3d.spec.ts` drives the
  Playground, gallery, trace panel, filter, params and failure paths against
  the release binary.
- **Follow-ups:** **17A** — six review carries (the matrix's unconfirmed
  Tripo3D `rig` cell, landing-generator docstrings, the corrected 404-retry
  rationale, transient-error tolerance in the trace-panel poll, an honest
  video-load fallback, Single Mode's model picker disabled mid-job).
  **17B** — four real-browser fixes: a corrupt GLB is told apart from a
  missing one (`parse` vs `load`, from `PerformanceResourceTiming.responseStatus`
  captured by a per-load `PerformanceObserver`), `ResultTile3D`
  done-without-mesh renders as an error, `interaction-prompt="none"`, and the
  inspector toolbar wraps by group. **17C** — two backend fixes: every family's
  extractor rejects another family's model with 400
  `model_media_type_mismatch`, and async request logs (video and 3D) leave
  `pending` on every terminal transition via
  `DatabaseStore::update_request_log_status`.

### 14.3 Not done, or done differently from §10–§13

- No vendor adapters, vendor wiremock tests, or `#[ignore]` live tests; no
  routing-strategy dispatch for 3D. All open.
- §10's "`cargo clippy --all-targets -- -D warnings` clean" does not hold on
  this branch: the remaining findings are in code this work did not write (the
  two in code it did were fixed in `2fed306`). The plan's Task 17 status
  records the blame for each.
- In local (no-S3) mode the asset store is an unbounded process-lifetime map,
  like the mock-video store before it; S3 deployments never use it.
