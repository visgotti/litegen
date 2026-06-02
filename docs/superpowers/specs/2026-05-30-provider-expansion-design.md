# Provider Expansion + Flexible Authentication — Design

**Status:** draft (awaiting review)
**Date:** 2026-05-30
**Scope:** litegen-core (+ models/, litegen.example.yaml, dashboard in final wave)
**Builds on:** [Capability Registry & Per-Model Param Handling](2026-05-28-capability-registry-design.md)

---

## 1. Problem & goal

litegen unifies image/video generation across providers, but currently integrates only OpenAI, Stability, Replicate, Google (image), Fal, Runway (video), Luma (video). The goal is to integrate **every** first-party vendor with a reachable public API, at true parity with the existing capability-registry model, including:

- New **modalities** for vendors we already have: **Google Veo** (video), **Luma Photon** (image), **Runway** (image).
- New **image+video** vendors: **Amazon Bedrock Nova**, **MiniMax/Hailuo**, **Kling**, **Leonardo.Ai**, **ByteDance** (Seedream/Seedance), **Tencent Hunyuan**.
- New **image-only** vendors: **Black Forest Labs** (FLUX direct), **Ideogram**, **Recraft**.
- New **video-only** vendors: **Vidu**, **Pixverse**.

(Stability hosted **video** is deliberately excluded — its hosted Stable Video API is discontinued; self-host only.)

Every vendor was confirmed to have a **self-serve first-party API** (research pass 2026-05-30; see §12 appendix for per-vendor facts and verbatim documentation quotes). Nothing is dropped as "unobtainable."

The central blocker is **authentication**: today a provider's auth is a single `api_key: String` (Bearer). The new vendors use **five distinct schemes** — custom-header API keys, query-param keys, AWS SigV4 request signing, Kling per-request JWT, and Tencent TC3-HMAC-SHA256 signing. So the heart of this design is a **flexible auth abstraction** expressible in YAML (and, in a final wave, the dashboard UI), plus the ~20 provider implementations built on it.

### Design decisions (locked with the user)

1. **Direct first-party** integration for every vendor with a configurable API. No aggregator routing. Skip only vendors with no obtainable API (none, as it turns out).
2. **Flexible auth** abstraction; both YAML-first config and (final wave) the dashboard can express any scheme.
3. Live integration tests gated by **`#[ignore]` + env-var self-skip**; normal `cargo test` stays fully offline.
4. **One spec, implemented in waves.**
5. YAML-first auth + all providers + live-test harness + an auth-schema endpoint **now**; DB-backed credential store + registry hot-reload + dashboard Providers page as the **final wave**.

### Non-goals (this spec)

- Aggregator (fal/Replicate) routing for the new vendors.
- Per-user/per-key quota or billing changes.
- Variable pricing beyond `base_cost_usd` (the existing registry field; per-second/per-credit notes captured for future price-api tie-in).
- Stability hosted video.

---

## 2. Architecture overview

Nothing about the capability-registry contract changes. Each new provider is an independent unit consisting of:

1. A Rust impl in `src/providers/{image,video}/<vendor>.rs` implementing `ImageProvider` / `VideoProvider`.
2. A declarative `models/<vendor>.yaml`.
3. Wiring: `providers/{image,video}/mod.rs` (`pub mod`), `proxy/registry.rs` (import + `register_provider` match arm), `config/mod.rs` (env-var map), `litegen.example.yaml`.
4. Doc comments carrying `@see <url>` references **and a verbatim quote** from the scraped docs (house convention; source material gathered in §12).

The **new shared machinery** (Wave 0) is:

- `src/providers/auth/` — the `AuthSpec` enum + signer/injector implementations.
- `ProviderInstanceConfig.credentials` + `ProviderEnvConfig` credential bag.
- Auth-aware registration skip-guard.
- `MaterializedRefForm::ProviderManagedUpload` for two-step ref upload (Leonardo, Pixverse).
- `GET /v1/providers/schema` endpoint.
- `tests/live/` harness + `.env.example`.

---

## 3. The `AuthSpec` abstraction

Auth scheme is **intrinsic to the vendor**; users supply only credentials. A typed enum is dispatched at request time; signing algorithms live in audited Rust.

`src/providers/auth/mod.rs`:

```rust
/// How a provider authenticates outbound requests. Declared per-provider
/// (constant in code), NOT user-configurable — users supply only credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum AuthSpec {
    /// API key placed in a request header. Covers Bearer (Authorization, "Bearer "),
    /// "Token " (Vidu), and raw custom headers (x-key, x-goog-api-key, Api-Key, API-KEY).
    Header { name: String, value_prefix: String },

    /// API key placed in a query parameter (Google ?key= alternative).
    QueryParam { name: String },

    /// AWS Signature V4 request signing (Bedrock). service = "bedrock".
    AwsSigV4 { service: String, default_region: String },

    /// Kling per-request JWT: HS256, payload {iss: access_key, exp: now+1800,
    /// nbf: now-5}, sent as `Authorization: Bearer <jwt>`.
    KlingJwt,

    /// Tencent Cloud TC3-HMAC-SHA256 request signing (Hunyuan). service e.g. "hunyuan"/"vclm".
    TencentTc3 { service: String, default_region: String },
}

/// Credentials a user supplies. The union across all schemes; each scheme reads
/// only the fields it needs. Sourced from YAML/env (Wave 0) or the DB (Wave 4).
#[derive(Debug, Clone, Default)]
pub struct ProviderCredentials {
    pub api_key: Option<String>,            // header/query/bearer schemes; Ark/MiniMax/etc.
    pub api_keys: Vec<ApiKeyEntry>,         // weighted multi-key (existing pool)
    pub key_id: Option<String>,             // SigV4 access_key_id; TC3 secret_id; Kling access_key
    pub key_secret: Option<String>,         // SigV4 secret_access_key; TC3 secret_key; Kling secret_key
    pub region: Option<String>,             // SigV4 / TC3 (non-secret; part of host & cred scope)
    pub extra: HashMap<String, String>,     // future aux (group_id, etc.)
}

impl AuthSpec {
    /// Required credential FIELD NAMES for this scheme — drives the skip-guard
    /// and the /v1/providers/schema endpoint.
    pub fn required_fields(&self) -> &'static [&'static str] {
        match self {
            AuthSpec::Header { .. } | AuthSpec::QueryParam { .. } => &["api_key"],
            AuthSpec::AwsSigV4 { .. } => &["key_id", "key_secret", "region"],
            AuthSpec::KlingJwt => &["key_id", "key_secret"],
            AuthSpec::TencentTc3 { .. } => &["key_id", "key_secret", "region"],
        }
    }

    pub fn is_satisfied_by(&self, c: &ProviderCredentials) -> bool { /* all required present */ }
}

/// Apply auth to an outbound request. Header/QueryParam mutate the builder;
/// SigV4/TC3 sign the (method, url, headers, body); KlingJwt mints a JWT.
pub fn apply(
    spec: &AuthSpec,
    creds: &ProviderCredentials,
    region_override: Option<&str>,
    parts: SignableRequest,   // method, url, headers, body bytes
) -> Result<reqwest::RequestBuilder, ProviderError>;
```

Module layout:

| Path | Purpose |
|---|---|
| `src/providers/auth/mod.rs` | `AuthSpec`, `ProviderCredentials`, `apply()`, `required_fields()` |
| `src/providers/auth/header.rs` | Header + QueryParam injectors |
| `src/providers/auth/sigv4.rs` | AWS SigV4 (`AWS4-HMAC-SHA256`, service `bedrock`) |
| `src/providers/auth/tc3.rs` | Tencent `TC3-HMAC-SHA256` (key seed `TC3`, scope `…/tc3_request`) |
| `src/providers/auth/kling_jwt.rs` | HS256 JWT mint (iss/exp/nbf) |

**Dependencies:** add `jsonwebtoken` (Kling). SigV4 and TC3 are implemented directly on the existing `hmac` + `sha2` crates — no AWS SDK — matching the repo's hand-rolled, lean-binary style. Both signers get exhaustive unit tests against the canonical examples in their official docs (AWS SigV4 test suite; Tencent TC3 worked example).

**Static vs dynamic auxiliary headers:** Runway's mandatory non-secret `X-Runway-Version: 2024-11-06` is set via existing `extra_headers`. Pixverse's per-request `Ai-trace-id` (fresh UUID per call) is added inside the provider's `generate()`.

### Provider trait touch-point

Each provider stores its `AuthSpec` (a constant) and `ProviderCredentials` (from `configure()`), and replaces today's `api_key()` + manual `.header("Authorization", …)` with `auth::apply(&self.auth, &self.creds, region, parts)`. Existing Bearer providers (OpenAI, Stability, …) migrate to `AuthSpec::Header { name: "Authorization", value_prefix: "Bearer " }` — behavior-preserving; covered by their existing wiremock tests.

---

## 4. Config & wiring changes

### 4.1 `ProviderEnvConfig` (config/mod.rs)

Add credential-bag fields (all optional, back-compatible):

```rust
pub struct ProviderEnvConfig {
    // existing:
    pub api_key: Option<String>,
    pub api_keys: Option<String>,
    pub api_base: Option<String>,
    pub model_mapping: HashMap<String, String>,
    pub extra_headers: HashMap<String, String>,
    pub options: Option<serde_json::Value>,
    pub enabled: bool,
    // new:
    pub key_id: Option<String>,        // access_key_id / secret_id / kling access_key
    pub key_secret: Option<String>,    // secret_access_key / secret_key
    pub region: Option<String>,
    #[serde(default)] pub credentials_extra: HashMap<String, String>,
}
```

`build_instance_config` (registry.rs) maps these into `ProviderInstanceConfig.credentials: ProviderCredentials`.

### 4.2 Auth-aware skip-guard (registry.rs `init_from_config`)

Today (`registry.rs:55`) a provider is skipped when `api_key.is_empty() && api_keys.is_empty()`. This wrongly drops Bedrock/Kling/Hunyuan (no bare `api_key`). Change to: look up the provider's declared `AuthSpec` and skip only if `!auth_spec.is_satisfied_by(&credentials)` (mock still exempt).

### 4.3 `register_provider` match arms

One arm per new vendor (image-only / video-only / image+video), following the existing pattern. New vendor names: `bedrock`, `minimax`, `kling`, `leonardo`, `bytedance`, `hunyuan`, `bfl`, `ideogram`, `recraft`, `vidu`, `pixverse`. `google`, `luma`, `runway` arms gain their second-modality registration.

### 4.4 Env-var mappings (config/mod.rs `load_provider_env_overrides`)

Add: `BFL_API_KEY`, `IDEOGRAM_API_KEY`, `RECRAFT_API_TOKEN`, `MINIMAX_API_KEY` (+`MINIMAX_API_BASE`), `BYTEDANCE_API_KEY`/`ARK_API_KEY` (+`_API_BASE`), `VIDU_API_KEY`, `PIXVERSE_API_KEY`, `LEONARDO_API_KEY`, `KLING_ACCESS_KEY`/`KLING_SECRET_KEY`, `BEDROCK_ACCESS_KEY_ID`/`BEDROCK_SECRET_ACCESS_KEY`/`BEDROCK_REGION` (fallback to `AWS_*`), `TENCENT_SECRET_ID`/`TENCENT_SECRET_KEY`/`TENCENT_REGION`. Update `litegen.example.yaml` with a commented block per provider.

### 4.5 `GET /v1/providers/schema`

New read endpoint returning, per registered provider: `name`, `auth.scheme`, `auth.required_fields`, and human labels. This is the contract the dashboard Providers page (Wave 4) renders forms from — built now so the UI wave is pure frontend.

---

## 5. Materializer additions

The existing `RefProviderFormat::{Url, Base64, Multipart}` (capability-registry §5) covers most vendors. Two additions:

- **`MaterializedRefForm::ProviderManagedUpload { bytes, content_type, filename }`** — for vendors that require a vendor-side upload returning an opaque ID used in the generate body: **Leonardo** (init-image presigned-S3 upload → `imageId`) and **Pixverse** (`POST /openapi/v2/image/upload` → integer `img_id`). The materializer hands raw bytes to the provider, which performs the upload step inside `generate()`.
- **Format enforcement per vendor:** Veo and Bedrock accept **base64 only** (no input URLs); Luma accepts **CDN URL only** (materializer must upload base64/blob refs to `proxy::storage` and pass the resulting URL). These are expressed as the model's `ref_inputs.provider_format` in YAML, so the existing validator/materializer pipeline enforces them — no provider-specific branching needed beyond the two upload cases above.

---

## 6. Provider-specific notes

- **Google Veo (video):** `POST /v1beta/models/{model}:predictLongRunning` → returns operation `name`; poll `GET /v1beta/{name}` until `response.done`; read `generateVideoResponse.generatedSamples[0].video.uri` (download with the API key). Reuses the existing `google` API key. base64 inline refs (first frame `instances[].image`, last frame `instances[].lastFrame`, up to 3 `referenceImages[]` on Veo 3.1).
- **Luma Photon (image):** `POST /generations/image` on the existing Luma host; poll `GET /generations/{id}`; `assets.image`. Refs are **CDN URLs only**.
- **Runway (image):** `POST /v1/text_to_image` (+ `X-Runway-Version`); poll `GET /v1/tasks/{id}`; `referenceImages[]` with `uri` (URL or base64 data-URI) + `tag` (referenced via `@tag` in `promptText`).
- **Bedrock Nova:** image (`amazon.nova-canvas-v1:0`) is **sync** `POST /model/{id}/invoke`, base64 image in/out. Video (`amazon.nova-reel-v1:1`) is **async** `POST /async-invoke` → `{invocationArn}`; poll `GET /async-invoke/{arn}`; **output is written to the caller's S3 bucket** (`outputDataConfig.s3OutputDataConfig.s3Uri`), not returned inline. Reuses existing `proxy::storage` S3 config. SigV4 primary (bearer-token alternative noted).
- **MiniMax:** image sync `POST /v1/image_generation`; video async `POST /v1/video_generation` → poll `GET /v1/query/video_generation` → `GET /v1/files/retrieve`. No GroupId needed on the `.io` endpoints. Region split via `api_base` (`api.minimax.io` vs `api.minimaxi.com`).
- **Kling:** image `POST /v1/images/generations` (poll `/{task_id}`); video `POST /v1/videos/text2video` & `/image2video` (+ `image_tail` last frame, `multi-image2video`, `video-extend`). Per-request HS256 JWT.
- **Leonardo:** image `POST /generations` (poll `GET /generations/{id}`); video `POST /generations-image-to-video` (MOTION2/VEO3/KLING enums) + legacy `/generations-motion-svd`. Refs by **server-side imageId** (presigned-S3 upload step).
- **ByteDance:** image (Seedream) sync `POST /api/v3/images/generations` (OpenAI-compatible); video (Seedance) async `POST /api/v3/contents/generations/tasks` → poll `/{task_id}`; video params passed as `--flags` in a text content block, refs via `image_url` blocks. Host via `api_base` (BytePlus intl vs Volcengine China).
- **Tencent Hunyuan:** **RPC-style** — POST to `/`, action via `X-TC-Action` header. Image: `hunyuan.tencentcloudapi.com`, `SubmitHunyuanImageJob`/`QueryHunyuanImageJob` (v `2023-09-01`); sync `TextToImageLite` variant. Video: `vclm.tencentcloudapi.com`, `SubmitImageToVideoJob`/`QueryImageToVideoJob` (v `2024-05-23`). `ap-guangzhou` only. TC3 signing.
- **BFL:** `POST /v1/flux-pro-1.1` (and flux-pro/dev/ultra/kontext/FLUX.2) → `{id, polling_url}`; **poll the returned `polling_url`** (not a hand-built path); `result.sample` URL (expires ~10 min). `x-key` header. Refs: `input_image` (Kontext, base64/URL), `image_prompt` (Redux, base64).
- **Ideogram:** sync `POST /v1/ideogram-v3/generate` (multipart or JSON); `Api-Key` header; reference images are **multipart file uploads** (style/character). Image-only.
- **Recraft:** sync `POST /v1/images/generations` (OpenAI-compatible base); Bearer; img2img/inpaint via separate multipart endpoints. Image-only.
- **Vidu:** async `POST /ent/v2/{text2video,img2video,reference2video,start-end2video}` → poll `GET /ent/v2/tasks/{id}/creations`; `Authorization: Token`. `reference2video` takes 1–7 refs. Video-only.
- **Pixverse:** async `POST /openapi/v2/video/{text,img,transition,extend}/generate` → poll `GET /openapi/v2/video/result/{id}`; `API-KEY` header + per-request `Ai-trace-id` UUID; refs via pre-upload (`/image/upload` → `img_id`). Video-only.

---

## 7. Live integration-test harness

New `litegen-core/tests/live/` directory + shared helper:

```rust
// tests/live/common.rs
pub fn key_or_skip(var: &str) -> Option<String> {
    match std::env::var(var) { Ok(v) if !v.is_empty() => Some(v), _ => None }
}
// Each live test:
#[tokio::test]
#[ignore = "live: requires real API credentials; run with --ignored"]
async fn live_bfl_flux_pro_generates() {
    let Some(key) = key_or_skip("BFL_API_KEY") else { eprintln!("skip: no BFL_API_KEY"); return; };
    /* real configure → generate → (poll) → assert non-empty bytes / valid URL */
}
```

- One real round-trip per provider/modality (generate → poll if async → assert output).
- `#[ignore]`d so `cargo test` stays fully offline and green; run with `cargo test -p litegen-core -- --ignored`. Tests self-skip (early return) when their credential env var is absent, so a partial key set runs only what it can.
- `.env.example` documents every credential variable (loaded via the existing `dotenvy`).
- The existing per-provider **wiremock mock tests** remain the offline correctness gate and ship for every new provider (asserting outbound body/headers/auth shape) — this is what catches silent-drop regressions in CI.

---

## 8. Wave plan

Each wave ends with `cargo test -p litegen-core` green and the shipped `models/*.yaml` parsing in the loader round-trip test.

- **Wave 0 — Foundation:** `auth/` module (enum + 4 signer/injector files + unit tests), `ProviderCredentials` + config bag, auth-aware skip-guard, `ProviderManagedUpload`, `GET /v1/providers/schema`, `tests/live/` scaffold + `.env.example`, migrate existing Bearer providers to `AuthSpec::Header`.
- **Wave 1 — Existing-vendor modalities:** Google Veo (video), Luma Photon (image), Runway (image).
- **Wave 2 — Header/Bearer new vendors:** BFL, Ideogram, Recraft, MiniMax, ByteDance, Vidu, Pixverse, Leonardo.
- **Wave 3 — Signing-heavy:** Kling (JWT), Bedrock (SigV4 + S3 video), Hunyuan (TC3).
- **Wave 4 — UI/runtime config:** DB-backed encrypted provider credentials + registry hot-reload + dashboard Providers page (consumes `/v1/providers/schema`).

---

## 9. Testing strategy

- **Unit:** every `AuthSpec` signer (SigV4 & TC3 against canonical doc examples; Kling JWT structure; header/query injection). Loader round-trip for each new `models/*.yaml`. `required_fields`/skip-guard logic.
- **Mock (wiremock), per provider:** outbound request body + auth header/signature shape; async create→poll→fetch happy path; error mapping. Reuses existing `ref_schema()`/`make_provider()`/`make_base()`/`make_extras()` helpers.
- **Live (`#[ignore]`):** one real round-trip per provider, run on demand once tokens exist.

---

## 10. Risks & mitigations

| Risk | Mitigation |
|---|---|
| SigV4/TC3 signers subtly wrong | Unit-test against each vendor's canonical signing example; live test confirms end-to-end. TC3 ≠ SigV4 (algo token, `tc3_request` terminator, `TC3` key seed) — separate impls. |
| Preview/changing model IDs (Veo 3.1, Seedance 2.0, Kling v3) | Pin stable GA IDs where available; treat preview IDs as medium-confidence in YAML (§12 flags them); model lists are data, easy to update. |
| Bedrock video needs caller S3 + region availability | Gate Bedrock *video* on S3 config; Bedrock *image* works without it. Document region constraints (Reel ≈ us-east-1). |
| Leonardo/Pixverse two-step ref upload | New `ProviderManagedUpload` form; upload handled in-provider with its own mock test. |
| Region-split hosts (MiniMax, ByteDance) / RPC style (Hunyuan) | `api_base` override; Hunyuan uses header-action dispatch, isolated in its provider. |
| Pricing figures drift / are per-second-or-credit | Store conservative `base_cost_usd` per model; capture per-unit notes in §12 for the future price-api tie-in; don't block on exact rates. |
| Vendor docs JS-rendered / anti-bot (Kling 446, Ideogram pricing) | Cross-checked against official SDKs + multiple sources (see §12 caveats); verify exact field schemas against live reference during implementation. |

---

## 11. Implementation order within a provider (template)

1. Write `models/<vendor>.yaml` (transcribe model list + params + ref_inputs from §12).
2. Loader round-trip test green.
3. Write the wiremock mock test(s) first (TDD) — assert outbound shape.
4. Implement the provider `generate()` (+`poll_status` for video) against the materialized request, using `auth::apply`. Add `@see` URLs + verbatim doc quote (from §12) to doc comments.
5. Wire mod.rs + registry arm + env map + example yaml.
6. Add the `#[ignore]` live test.
7. `cargo test -p litegen-core` green.

---

## 12. Appendix — per-vendor API facts & verbatim doc quotes

> Gathered 2026-05-30 via a 19-agent research pass with adversarial verification on the signing-heavy vendors. Each entry: obtainability, base URL, auth, endpoints/flow, model IDs, ref support, pricing, doc URLs, and exact quotes to embed in implementation comments.

### Google Veo (video) — provider `google` (extend)
- **Obtainable:** yes_self_serve · keys at `https://aistudio.google.com/apikey`
- **Base/auth:** `https://generativelanguage.googleapis.com/v1beta` · `x-goog-api-key` header (or `?key=`)
- **Video:** async — `POST /v1beta/models/{model}:predictLongRunning` → poll `GET /v1beta/{operation_name}` → `response.generateVideoResponse.generatedSamples[0].video.uri`
- **Models:** `veo-3.1-generate-preview`, `veo-3.1-fast-generate-preview`, `veo-3.0-generate-001`, `veo-3.0-fast-generate-001`, `veo-2.0-generate-001`
- **Refs:** base64 inline only — first frame `instances[].image.inlineData`, last frame `instances[].lastFrame`, up to 3 `referenceImages[]` (Veo 3.1). No input URLs.
- **Pricing:** per-second; Veo 3 Standard $0.40/s, Veo 3 Fast $0.10/s (720p), Veo 2 $0.35/s; clips 4/6/8s; paid-tier only.
- **Docs:** ai.google.dev/gemini-api/docs/video, /docs/pricing
- **Quotes:**
  - `curl -s "${BASE_URL}/models/veo-3.1-generate-preview:predictLongRunning" \` — ai.google.dev/gemini-api/docs/video
  - `-H "x-goog-api-key: $GEMINI_API_KEY" \` — same
  - `status_response=$(curl -s -H "x-goog-api-key: $GEMINI_API_KEY" "${BASE_URL}/${operation_name}")` — same
- **Caveats:** veo-3.1* are preview/paid-tier; reference images + extension are 3.1-only; Developer API takes base64 only (Vertex AI alt supports GCS URIs).

### Luma Photon (image) — provider `luma` (extend)
- **Obtainable:** yes_self_serve · keys at `lumalabs.ai/dream-machine/api/keys`
- **Base/auth:** `https://api.lumalabs.ai/dream-machine/v1` · Bearer
- **Image:** async — `POST /generations/image` → poll `GET /generations/{id}` (`assets.image`); optional `sync` boolean.
- **Models:** `photon-1`, `photon-flash-1`
- **Refs:** **CDN URL only** — `image_ref` (≤4 {url,weight}), `style_ref`, `character_ref.identity0.images[]`, `modify_image_ref`. No base64/multipart.
- **Pricing:** Photon $0.015/1080p image; Flash $0.002/image.
- **Docs:** docs.lumalabs.ai/docs/image-generation, /reference/generateimage
- **Quotes:**
  - `post https://api.lumalabs.ai/dream-machine/v1/generations/image` — /reference/generateimage
  - `You can choose from our two model versions: photon-1 (default) photon-flash-1` — /docs/image-generation
  - `You should upload and use your own cdn image urls, currently this is the only way to pass an image` — /docs/image-generation
- **Caveats:** confirm `character_ref.identity0` exact key against live reference.

### Runway (image) — provider `runway` (extend)
- **Obtainable:** yes_self_serve · `dev.runwayml.com`
- **Base/auth:** `https://api.dev.runwayml.com/v1` · Bearer + mandatory `X-Runway-Version: 2024-11-06`
- **Image:** async — `POST /v1/text_to_image` → poll `GET /v1/tasks/{id}`
- **Models:** `gen4_image`, `gen4_image_turbo` (turbo requires ≥1 ref), + others
- **Refs:** `referenceImages[]` {`uri` (URL or base64 data-URI), `tag`}; tag referenced via `@tag` in `promptText`.
- **Pricing:** credits @ $0.01; gen4_image 5cr/720p ($0.05) or 8cr/1080p ($0.08); turbo 2cr ($0.02). $10 min prepaid.
- **Docs:** docs.dev.runwayml.com/guides/using-the-api/, /api/
- **Quotes:**
  - `curl -X POST https://api.dev.runwayml.com/v1/text_to_image` — /guides/using-the-api/
  - `"promptText": "@EiffelTower painted in the style of @StarryNight", "model": "gen4_image", "ratio": "1920:1080",` — same
  - `-H "Authorization: Bearer $RUNWAYML_API_SECRET" -H "X-Runway-Version: 2024-11-06"` — same

### Amazon Bedrock Nova — provider `bedrock` (new, image+video)
- **Obtainable:** yes_self_serve · `console.aws.amazon.com/bedrock`
- **Base/auth:** `https://bedrock-runtime.{region}.amazonaws.com` · AWS SigV4 (service `bedrock`); bearer-token alt (`AWS_BEARER_TOKEN_BEDROCK`)
- **Image (sync):** `POST /model/amazon.nova-canvas-v1:0/invoke`; JSON `taskType` + `imageGenerationConfig`; base64 image(s) in response.
- **Video (async→S3):** `POST /async-invoke` `{modelId, modelInput, outputDataConfig}` → `{invocationArn}`; poll `GET /async-invoke/{arn}`; output `video.mp4` lands in caller's S3.
- **Models:** `amazon.nova-canvas-v1:0`; `amazon.nova-reel-v1:1`, `amazon.nova-reel-v1:0`
- **Refs:** base64-in-JSON only. Canvas taskTypes: TEXT_IMAGE (+conditionImage CANNY/SEGMENTATION), COLOR_GUIDED, IMAGE_VARIATION, INPAINTING/OUTPAINTING (mask), BACKGROUND_REMOVAL, VIRTUAL_TRY_ON. Reel: `textToVideoParams.images` single starting keyframe (1280×720); no last_frame.
- **Pricing:** Canvas per-image (tiered); Reel per-second.
- **Docs:** docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_{InvokeModel,StartAsyncInvoke,GetAsyncInvoke}.html; /nova/latest/userguide/{image,video}-gen-access.html
- **Quotes:**
  - `POST /model/{modelId}/invoke HTTP/1.1` — API_runtime_InvokeModel
  - `POST /async-invoke HTTP/1.1 … {"clientRequestToken","modelId","modelInput","outputDataConfig","tags"}` — API_runtime_StartAsyncInvoke
  - `modelId (Required) … For Amazon Nova Reel, this is "amazon.nova-reel-v1:1"` — nova video-gen-access
  - `Access to all Amazon Bedrock foundation models is enabled by default …` — bedrock model-access
- **Caveats:** region availability varies (Reel ≈ us-east-1); SigV4 safest for the S3 async path; Reel has only a starting keyframe (emulate first+last client-side if needed).

### MiniMax / Hailuo — provider `minimax` (new, image+video)
- **Obtainable:** yes_self_serve · `platform.minimax.io` (intl) / `platform.minimaxi.com` (China)
- **Base/auth:** `https://api.minimax.io/v1` (or `api.minimaxi.com`) · Bearer; **no GroupId** needed on `.io` for these endpoints.
- **Image (sync):** `POST /v1/image_generation` — model `image-01`
- **Video (async):** `POST /v1/video_generation` → poll `GET /v1/query/video_generation` → `GET /v1/files/retrieve`
- **Models:** image `image-01`; video `MiniMax-Hailuo-2.3`, `MiniMax-Hailuo-02`, `T2V-01`, `T2V-01-Director`, `S2V-01`
- **Refs:** image `subject_reference` (URL or base64); video `first_frame_image`/`last_frame_image` (Hailuo-02), `subject_reference` (S2V-01).
- **Pricing:** token-plan; per-call varies; video URLs expire ~9h.
- **Docs:** platform.minimax.io/docs/guides/{image,video}-generation, /docs/api-reference/video-generation-{t2v,fl2v}
- **Quotes:**
  - `headers = {"Authorization": f"Bearer {api_key}"} … payload = {"model": "image-01", … } endpoint https://api.minimax.io/v1/image_generation` — guides/image-generation
  - `first_frame_image: … Supports public URLs or Base64-encoded Data URLs … last_frame_image … Supported Model: MiniMax-Hailuo-02` — api-reference/video-generation-fl2v
- **Caveats:** API key bound to its region host; GroupId possibly still needed on China host (unverified).

### Kling — provider `kling` (new, image+video)
- **Obtainable:** yes_self_serve · `app.klingai.com/global/dev`
- **Base/auth:** `https://api.klingai.com` (regional e.g. `api-singapore.klingai.com`) · **JWT HS256** from `access_key`+`secret_key` (payload `iss=ak, exp=now+1800, nbf=now-5`), sent as `Authorization: Bearer <jwt>`
- **Image (async):** `POST /v1/images/generations` → `GET /v1/images/generations/{task_id}`
- **Video (async):** `POST /v1/videos/text2video`, `/image2video` (+ `image_tail` last frame), `/multi-image2video` (≤4 refs), `/video-extend` → matching `GET …/{task_id}`
- **Models:** image `kling-v1`/`-v1-5`/`-v2`; video `kling-v1`…`v2-1`/`v3`
- **Refs:** `image` first frame + `image_tail` last frame (URL or base64); multi-image up to 4.
- **Pricing:** prepaid Resource Packages; per-gen by tier(std/pro)/res/duration.
- **Docs:** app.klingai.com/global/dev/document-api/... ; verified via official Node SDK `github.com/aself101/kling-api`
- **Quotes:**
  - `header = { alg: 'HS256', typ: 'JWT' }; payload = { iss: accessKey, exp: now+1800, nbf: now-5 }` — aself101/kling-api src/auth.ts
  - `'POST', '/v1/videos/text2video' … '/v1/videos/image2video' … '/v1/videos/video-extend' … '/v1/videos/multi-image2video'` — src/operations/video.ts
  - `'POST', '/v1/images/generations'` — src/operations/image.ts
- **Caveats:** docs host anti-bot (446); confirm exact `model_name` strings + base host (regional) in console; prepaid balance gates production.

### Leonardo.Ai — provider `leonardo` (new, image+video)
- **Obtainable:** yes_self_serve · `app.leonardo.ai`; $5 free API credit
- **Base/auth:** `https://cloud.leonardo.ai/api/rest/v1` · Bearer (`authorization` header)
- **Image (async):** `POST /generations` → poll `GET /generations/{id}`
- **Video (async):** `POST /generations-image-to-video` (MOTION2/MOTION2FAST/VEO3/VEO3FAST/VEO3_1/KLING2_1/KLING2_5) + legacy `POST /generations-motion-svd` → poll `GET /generations/{id}`
- **Models:** image `modelId` UUID (default `b24e16ff-06e3-43eb-8d33-4416c2d75876`; list via `GET /platformModels`); video enums above
- **Refs:** by **server-side imageId** — `init_image_id`/`init_generation_image_id`, controlnets, `imagePrompts`; video `imageId`+`imageType` (UPLOADED|GENERATED), `endFrameImage`. Upload via presigned-S3 (`ProviderManagedUpload`).
- **Pricing:** API credits in $; ~$0.002/std gen; Veo3 clips much higher. Verify via calculator.
- **Docs:** docs.leonardo.ai/reference/{creategeneration,getgenerationbyid,createimagetovideogeneration}
- **Quotes:**
  - `curl --request POST --url https://cloud.leonardo.ai/api/rest/v1/generations-image-to-video --header 'authorization: Bearer <YOUR_API_KEY>' … "model": "VEO3", "imageType": "UPLOADED", "resolution": "RESOLUTION_720", "duration": 8` — docs/generate-with-veo3...
  - `POST https://cloud.leonardo.ai/api/rest/v1/generations` — reference/creategeneration
  - `GET https://cloud.leonardo.ai/api/rest/v1/generations/{id}` — reference/getgenerationbyid
- **Caveats:** verify VEO3_1 enum + status strings on live schema; init images require separate upload step.

### ByteDance Seedream/Seedance — provider `bytedance` (new, image+video)
- **Obtainable:** yes_self_serve · Volcengine Ark (China) / BytePlus ModelArk (intl)
- **Base/auth:** `https://ark.ap-southeast.bytepluses.com/api/v3` (intl) or `https://ark.cn-beijing.volces.com/api/v3` (China) · Bearer (`$ARK_API_KEY`)
- **Image (sync, OpenAI-compatible):** `POST /api/v3/images/generations`
- **Video (async task):** `POST /api/v3/contents/generations/tasks` → poll `GET …/tasks/{task_id}`; params as `--flags` in text content block; refs via `image_url` blocks.
- **Models:** image `seedream-4-0-250828`, `seedream-3-0-t2i-250415`, `seededit-3-0-i2i-250628` (Doubao prefix on China); video `doubao-seedance-1-0-pro-250528`, `-lite-t2v/-i2v-250428`, `-2-0-260128`(+fast, medium-confidence)
- **Refs:** image-to-image / multi-ref via `image` (URL or base64); video first/last frame via `image_url` blocks.
- **Pricing:** Seedream 4.0 ~$0.03/image; Seedance 2.0 ~$0.93/5s 1080p (3rd-party).
- **Docs:** docs.byteplus.com/en/docs/ModelArk/{1298459,1541523,1824718}
- **Quotes:**
  - `The data plane API base URL is https://ark.ap-southeast.bytepluses.com/api/v3 ; … "Authorization: Bearer $ARK_API_KEY"` — ModelArk/1298459
  - `POST … /api/v3/contents/generations/tasks ; GET … /api/v3/contents/generations/tasks/{task_id} ; Authorization: Bearer YOUR_ARK_API_KEY` — apidog (cross-ref)
  - `seedream-4.0 Model ID: seedream-4-0-250828 … Pricing: $0.03 USD per generated image; No charge for failed generations.` — ModelArk/1824718
- **Caveats:** video is task-based (not OpenAI-compat); Seedance 2.0 IDs medium-confidence; control-plane AK/SK signing is separate and irrelevant to generation.

### Tencent Hunyuan — provider `hunyuan` (new, image+video)
- **Obtainable:** yes_self_serve (real-name auth; intl friction → closer to yes_with_approval for non-China) · keys at `console.cloud.tencent.com/cam/capi`
- **Base/auth:** image `https://hunyuan.tencentcloudapi.com/`, video `https://vclm.tencentcloudapi.com/` · **TC3-HMAC-SHA256** (`secret_id`+`secret_key`+region); RPC-style (POST `/`, action via `X-TC-Action`).
- **Image (async):** `X-TC-Action: SubmitHunyuanImageJob` / `QueryHunyuanImageJob` (v `2023-09-01`); sync `TextToImageLite` variant.
- **Video (async):** `X-TC-Action: SubmitImageToVideoJob` / `QueryImageToVideoJob` (v `2024-05-23`) on vclm.
- **Models:** image `SubmitHunyuanImageJob`/`…ChatJob`/`TextToImageLite`; video image-to-video/text-to-video/dance/sing/Kling-branded actions
- **Refs:** image optional ref via image field; video `ImageUrl`/`ImageBase64` first frame.
- **Pricing:** per-call; image 1 concurrent (Lite 3).
- **Docs:** tencentcloud.com/document/product/845/32207 (TC3); cloud.tencent.com/document/product/1729/105969 (image), /1616 (video); official Node SDK vclm_client.ts
- **Quotes:**
  - `TC3-HMAC-SHA256 … Authorization: TC3-HMAC-SHA256 Credential=SecretId/CredentialScope … scope … Date/service/tc3_request … signing key … prepending 'TC3' to your SecretKey.` — product/845/32207
  - `本接口仅支持其中的: ap-guangzhou … X-TC-Action: SubmitHunyuanImageJob, X-TC-Version: 2023-09-01, domain hunyuan.tencentcloudapi.com, POST to path /.` — product/1729/105969
  - `endpoint 'vclm.tencentcloudapi.com' … apiVersion '2024-05-23' … SubmitImageToVideoJob …` — vclm_client.ts
- **Caveats:** TC3 ≠ SigV4 (algo token / `tc3_request` / `TC3` key seed); `ap-guangzhou` only; per-action body fields inferred — verify on implementation.

### Black Forest Labs (FLUX) — provider `bfl` (new, image)
- **Obtainable:** yes_self_serve · `dashboard.bfl.ai/get-started`
- **Base/auth:** `https://api.bfl.ai` (regional `api.eu.bfl.ai`/`api.us.bfl.ai`) · `x-key` header
- **Image (async):** `POST /v1/flux-pro-1.1` (+ flux-pro/dev/ultra/kontext-pro/max/fill, FLUX.2 flux-2-*) → `{id, polling_url}`; **poll returned `polling_url`** (or `GET /v1/get_result?id=`); `result.sample` (expires ~10 min).
- **Models:** `flux-pro-1.1`, `flux-pro`, `flux-dev`, `flux-pro-1.1-ultra`, `flux-kontext-pro/max`, `flux-pro-1.0-fill`, `flux-2-pro/max/flex`
- **Refs:** `input_image` (Kontext edit, base64 or URL, ≤20MB), `image_prompt` (Redux, base64).
- **Pricing:** credits @ $0.01/image.
- **Docs:** docs.bfl.ai/flux_models/flux_1_1_pro, /kontext/kontext_image_editing, /api-reference/utility/get-result
- **Quotes:**
  - `curl -X POST 'https://api.bfl.ai/v1/flux-pro-1.1' -H "x-key: ${BFL_API_KEY}" … '{ "prompt": "...", "width": 1024, "height": 1024 }'` — flux_models/flux_1_1_pro
  - `echo "Image ready: $(echo $result | jq -r .result.sample)"` — same
  - `{ "prompt": "...", "input_image": "<base64 converted image>" }` — kontext/kontext_image_editing
- **Caveats:** no first-party video; verify US host subdomain; kontext-pro/max now "legacy" vs FLUX.2.

### Ideogram — provider `ideogram` (new, image)
- **Obtainable:** yes_self_serve · `ideogram.ai/manage-api`
- **Base/auth:** `https://api.ideogram.ai` · `Api-Key` header (not Bearer)
- **Image (sync):** `POST /v1/ideogram-v3/generate` (multipart or JSON); legacy `/generate`; also remix/inpaint/reframe/replace-background/describe/upscale.
- **Models:** Ideogram 3.0 (`rendering_speed` FLASH/TURBO/DEFAULT/QUALITY); legacy `V_2`/`V_2_TURBO`/`V_2A`...
- **Refs:** **multipart files only** (v3): `style_reference_images[]`, `character_reference_images[]` (+ mask); edit/inpaint image+mask.
- **Pricing:** per-output-image flat fee (~$0.03 TURBO / $0.06 DEFAULT / $0.09 QUALITY, 3rd-party).
- **Docs:** developer.ideogram.ai/api-reference/api-reference/generate-v3, /llms-full.txt
- **Quotes:**
  - `curl -X POST https://api.ideogram.ai/v1/ideogram-v3/generate -H "Api-Key: <apiKey>" -H "Content-Type: application/json" -d '{"prompt": "A picture of a cat"}'` — llms-full.txt
  - `You can now click the 'Create API key' button to generate your first API key.` — docs.ideogram.ai/plans-and-pricing/ideogram-api
  - `A black and white image of the same size as the image being edited (max size 10MB)` — api-reference/edit-v3
- **Caveats:** image-only; signed output URLs expire (download+store); v3 refs are file uploads only.

### Recraft — provider `recraft` (new, image)
- **Obtainable:** yes_self_serve · `app.recraft.ai/profile/api` (requires positive prepaid units to mint key)
- **Base/auth:** `https://external.api.recraft.ai/v1` · Bearer (`RECRAFT_API_TOKEN`)
- **Image (sync):** `POST /v1/images/generations` (+ `/raster`, `/vector`)
- **Models:** `recraftv3`(+`_vector`), `recraftv2`(+`_vector`), `recraftv4_1`(default)/`_pro`(+vector), `recraftv4`...
- **Refs:** separate multipart endpoints — `/v1/images/imageToImage`, `/inpaint`, `/replaceBackground`, `/generateBackground` (file uploads, not URL/base64).
- **Pricing:** units (1000=$1); recraftv3 $0.04 raster / $0.08 vector; recraftv4_1_pro $0.25/$0.30.
- **Docs:** recraft.ai/docs/api-reference/{getting-started,endpoints}.md
- **Quotes:**
  - `'https://external.api.recraft.ai/v1'` — getting-started.md
  - `Authorization: Bearer RECRAFT_API_TOKEN` — getting-started.md
  - `POST https://external.api.recraft.ai/v1/images/generations` — endpoints.md
- **Caveats:** image-only (video is web-UI only); default model now recraftv4_1.

### Vidu — provider `vidu` (new, video)
- **Obtainable:** yes_self_serve · `platform.vidu.com`
- **Base/auth:** `https://api.vidu.com` · `Authorization: Token <key>`
- **Video (async):** `POST /ent/v2/{text2video,img2video,reference2video,start-end2video}` → poll `GET /ent/v2/tasks/{id}/creations`
- **Models:** `viduq1`, `viduq1-classic`, `vidu2.0`, `viduq2`(+turbo/pro/pro-fast), `viduq3`(+turbo/pro/pro-fast/mix)
- **Refs:** `images[]` — img2video 1 (first frame), start-end2video 2 ([start,end]), reference2video 1–7; URL or base64.
- **Pricing:** credits @ $0.005 ($10 min); off-peak 50%.
- **Docs:** platform.vidu.com/docs/{text-to-video,get-generation}
- **Quotes:**
  - `POST https://api.vidu.com/ent/v2/text2video` — docs/text-to-video
  - `Authorization: Token {your api key}` — same
  - `https://api.vidu.com/ent/v2/tasks/{id}/creations` — docs/get-generation
- **Caveats:** video-only (no t2i); do NOT use the unrelated `vidu.io`/`viduhq/api-docs` product.

### Pixverse — provider `pixverse` (new, video)
- **Obtainable:** yes_self_serve · `platform.pixverse.ai`
- **Base/auth:** `https://app-api.pixverse.ai` · `API-KEY` header + per-request `Ai-trace-id` (fresh UUID)
- **Video (async):** `POST /openapi/v2/video/{text,img,transition,extend}/generate` → poll `GET /openapi/v2/video/result/{video_id}`
- **Models:** `v3.5`, `v4`, `v4.5`, `v5`, `v5.5`, `v5.6`, `v6`, `c1`
- **Refs:** pre-upload via `POST /openapi/v2/image/upload` (multipart `image` or `image_url`) → integer `img_id`; image-to-video `img_id`; transition `first_frame_img`/`last_frame_img` (v3.5/v4/v4.5). `ProviderManagedUpload`.
- **Pricing:** credits per video (V5: 720p/5s = 60cr, 1080p/5s = 120cr); $10/1000cr.
- **Docs:** docs.platform.pixverse.ai/{how-does-the-api-work,text-to-video-generation,upload-image,transition...}
- **Quotes:**
  - `A different Ai-trace-id for each unique request … If you use the same ai-trace-id multiple times, You won't get a new video generated` — how-does-the-api-work
  - `Your API key will only be displayed once. Make sure to copy it and store it securely.` — how-to-get-api-key
  - `"first_frame_img": 0, "last_frame_img": 0` — transition/first-last-frame
- **Caveats:** video-only (image endpoint is upload-only); status 1=success/5=processing/7=moderation/8=failed; some errors in Chinese; envelope `ErrCode/ErrMsg/Resp`.
