# LiteGen SDKs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship first-party TypeScript and Python SDKs for the LiteGen HTTP API, with full type/enum parity to `litegen-core` via OpenAPI-driven codegen.

**Architecture:** Add `ToSchema` coverage + OpenAPI aggregator to `litegen-core` so it serves a complete `/openapi.json`. Use `openapi-typescript` (TS, types-only) and `openapi-python-client` (Python, pydantic v2 + httpx) to generate low-level types/clients. Wrap them with hand-written ergonomic facades (`LiteGenClient` / `AsyncLiteGenClient`) that expose namespaced methods (`client.images.generate`, `client.videos.waitForCompletion`, etc.). Generated code is committed; a CI guard prevents drift.

**Tech Stack:**
- Rust (`litegen-core`): utoipa 5 (existing), axum, serde, chrono, uuid
- TypeScript SDK: openapi-typescript, tsup, vitest, native fetch, Node 18+/browsers/Deno/Bun
- Python SDK: openapi-python-client, pydantic v2, httpx, pytest, pytest-asyncio, respx, Python 3.10+

**Spec:** [docs/superpowers/specs/2026-05-28-litegen-sdks-design.md](../specs/2026-05-28-litegen-sdks-design.md)

---

## Phase 1 — Prerequisite work in `litegen-core`

### Task 1: Add `ToSchema` to capability schema types

**Why:** `GET /v1/models/{id}` returns `ModelSchema`, but `ModelSchema` and its sub-types only derive `Serialize, Deserialize`. Without `ToSchema`, the generated OpenAPI spec will have `unknown`/`any` for these payloads.

**Files:**
- Modify: `litegen-core/src/capabilities/schema.rs`
- Modify: `litegen-core/Cargo.toml` (verify utoipa is already a dep — it is)

- [ ] **Step 1: Add `utoipa::ToSchema` derives**

In [litegen-core/src/capabilities/schema.rs](litegen-core/src/capabilities/schema.rs), update every public struct/enum to derive `utoipa::ToSchema` in addition to its current derives. Specifically:

```rust
// Add `utoipa::ToSchema` to the derive list on each of:
// - MediaType
// - ModelCapabilityFlags
// - ModelPricing
// - PromptSpec
// - ParamSpec
// - SizeSpec
// - RefInputSpec
// - RefRoleSpec
// - RefProviderFormat
// - ModelSchema
// - ModelsFile
```

Example for the first one:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
}
```

Apply the same pattern to all the types listed.

Note: `ParamSpec` and `SizeSpec` and `RefProviderFormat` use `#[serde(tag = "...")]` discriminated unions. utoipa supports these via `#[schema(...)]` if needed, but for now just adding `ToSchema` should work — the generated schema will use `anyOf` discriminators that codegen handles fine.

- [ ] **Step 2: Update `get_model_schema` to declare its response body**

In [litegen-core/src/api/handlers.rs](litegen-core/src/api/handlers.rs), find the `#[utoipa::path]` on `get_model_schema` and add the body type:

Before:
```rust
#[utoipa::path(
    get,
    path = "/v1/models/{id}",
    responses(
        (status = 200, description = "Model schema"),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Models"
)]
```

After:
```rust
#[utoipa::path(
    get,
    path = "/v1/models/{id}",
    responses(
        (status = 200, description = "Model schema", body = crate::capabilities::ModelSchema),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Models"
)]
```

- [ ] **Step 3: Build to verify nothing broke**

Run: `cargo build -p litegen-core`
Expected: Success.

- [ ] **Step 4: Commit**

```bash
git add litegen-core/src/capabilities/schema.rs litegen-core/src/api/handlers.rs
git commit -m "feat(litegen-core): add ToSchema to capability types for OpenAPI codegen"
```

---

### Task 2: Replace ad-hoc JSON responses with typed structs

**Why:** Several handlers return `serde_json::json!({...})` inline, which produces untyped objects in the OpenAPI spec. Each needs a real struct with `Serialize + ToSchema`.

**Files:**
- Modify: `litegen-core/src/types/mod.rs` (add new response types)
- Modify: `litegen-core/src/api/handlers.rs` (use typed responses)

- [ ] **Step 1: Add typed response structs to `types/mod.rs`**

Append to [litegen-core/src/types/mod.rs](litegen-core/src/types/mod.rs):

```rust
// ─── Typed API Response Wrappers ────────────────────────────────────────────

/// Response for `GET /health/live`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LivenessResponse {
    pub status: String,
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
}

/// Response for `GET /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyListResponse {
    pub data: Vec<ApiKeyInfo>,
}

/// Response for `POST /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyCreatedResponse {
    pub key: String,
    pub prefix: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
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
```

- [ ] **Step 2: Update handlers to use the new typed responses**

In [litegen-core/src/api/handlers.rs](litegen-core/src/api/handlers.rs):

Replace `liveness`:
```rust
pub async fn liveness() -> impl IntoResponse {
    Json(LivenessResponse { status: "ok".to_string() })
}
```

Add `#[utoipa::path]` to `liveness`:
```rust
#[utoipa::path(
    get,
    path = "/health/live",
    responses((status = 200, description = "Liveness probe", body = LivenessResponse)),
    tag = "System"
)]
pub async fn liveness() -> impl IntoResponse { /* as above */ }
```

Replace `health_check` body construction (keep the status logic, swap the JSON):
```rust
pub async fn health_check(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let health = state.router.registry.health_check_all().await;
    let all_healthy = health.iter().all(|h| h.healthy);
    let status_code = if all_healthy || health.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = HealthResponse {
        status: if all_healthy { "healthy".into() } else { "degraded".into() },
        providers: health,
        cache: CacheStatus {
            enabled: state.router.cache.is_enabled(),
            entries: state.router.cache.entry_count() as u64,
        },
    };
    (status_code, Json(body))
}
```

Update `#[utoipa::path]` on `health_check`:
```rust
#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Health check results", body = HealthResponse)),
    tag = "System"
)]
```

Replace `list_models` body:
```rust
pub async fn list_models(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let models: Vec<ModelInfo> = state.registry.all()
        .map(project_model_info)
        .collect();
    Json(ModelListResponse { object: "list".into(), data: models })
}
```

Update `#[utoipa::path]` to declare `body = ModelListResponse`.

Replace `create_api_key` success arm:
```rust
Ok(key) => (
    StatusCode::CREATED,
    Json(ApiKeyCreatedResponse {
        key: raw_key,
        prefix: key.key_prefix,
        name: key.name,
        created_at: key.created_at,
    }),
)
    .into_response(),
```

Update `#[utoipa::path]` to declare `body = ApiKeyCreatedResponse`.

Replace `list_api_keys` success body:
```rust
Ok(keys) => Json(ApiKeyListResponse {
    data: keys.into_iter().map(|k| ApiKeyInfo {
        id: k.id,
        name: k.name,
        prefix: k.key_prefix,
        created_at: k.created_at,
        expires_at: k.expires_at,
        is_active: k.is_active,
    }).collect(),
}).into_response(),
```

Update `#[utoipa::path]` to `body = ApiKeyListResponse`.

Replace `revoke_api_key`:
```rust
Ok(true) => (StatusCode::OK, Json(RevokeKeyResponse { revoked: true })).into_response(),
Ok(false) => (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
```

Update `#[utoipa::path]` to `body = RevokeKeyResponse` for 200.

Replace `clear_cache`:
```rust
pub async fn clear_cache(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    state.router.cache.clear().await;
    Json(CacheClearedResponse { cleared: true })
}
```

Update `#[utoipa::path]` to `body = CacheClearedResponse`.

- [ ] **Step 3: Build and run existing tests**

Run: `cargo build -p litegen-core && cargo test -p litegen-core`
Expected: All existing tests pass. (Tests that asserted on response shape may need adjustment if they did so structurally rather than via JSON value matching — fix them if so.)

- [ ] **Step 4: Commit**

```bash
git add litegen-core/src/types/mod.rs litegen-core/src/api/handlers.rs
git commit -m "feat(litegen-core): replace ad-hoc JSON responses with typed structs"
```

---

### Task 3: Add `GET /v1/videos/{id}` status endpoint

**Why:** Video generation is async; the SDK polling helper requires a way to fetch current status by local ID.

**Files:**
- Modify: `litegen-core/src/proxy/router.rs` (add job tracking + `get_video_status`)
- Modify: `litegen-core/src/api/handlers.rs` (add `get_video_status` handler)
- Modify: `litegen-core/src/proxy/router_tests.rs` (add test for the new method)

- [ ] **Step 1: Add in-memory job tracking to the router**

In [litegen-core/src/proxy/router.rs](litegen-core/src/proxy/router.rs), at the top with the other imports, add:

```rust
use tokio::sync::RwLock;
```

Find the `Router` struct definition. Add a new field:

```rust
pub struct Router {
    // ... existing fields ...
    pub video_jobs: Arc<RwLock<std::collections::HashMap<String, VideoGenerationHandle>>>,
}
```

In the `Router::new` (or equivalent constructor) — find where Router is constructed — add:
```rust
video_jobs: Arc::new(RwLock::new(std::collections::HashMap::new())),
```

- [ ] **Step 2: Persist the handle in `generate_video`**

In [litegen-core/src/proxy/router.rs](litegen-core/src/proxy/router.rs), inside `pub async fn generate_video`, find the point where the local `id` is generated (currently `format!("litegen-vid-{}", uuid::Uuid::new_v4())`). Refactor so the id is generated before the response is built, then insert into the map:

```rust
let local_id = format!("litegen-vid-{}", uuid::Uuid::new_v4());
// `_handle` is the destructured VideoGenerationHandle from earlier in the function;
// rename it to `handle` so we can use it.
self.video_jobs.write().await.insert(local_id.clone(), handle.clone());

Ok(VideoGenerationResponse {
    id: local_id,
    // ... rest unchanged ...
})
```

If the existing destructuring is `let (provider_name, _handle) = ...`, change `_handle` to `handle` and use `handle.clone()` for the insert.

- [ ] **Step 3: Add `get_video_status` to the router**

Append to the `impl Router` block in `router.rs`:

```rust
/// Look up an in-flight video generation by local ID and poll its provider.
#[tracing::instrument(skip(self), fields(id = %id))]
pub async fn get_video_status(&self, id: &str) -> Result<VideoGenerationResponse, ProxyError> {
    let handle = {
        let jobs = self.video_jobs.read().await;
        jobs.get(id).cloned()
    };
    let handle = handle.ok_or_else(|| ProxyError::NotFound(format!("video job '{}' not found", id)))?;

    let provider = self.registry
        .video_provider_for(&handle.provider)
        .await
        .ok_or_else(|| ProxyError::ProviderNotConfigured(handle.provider.clone()))?;

    let poll = provider.poll_status(&handle).await.map_err(ProxyError::from)?;

    // If terminal, drop from the map.
    if matches!(poll.status,
        crate::types::GenerationStatus::Completed
        | crate::types::GenerationStatus::Failed
        | crate::types::GenerationStatus::Cancelled)
    {
        self.video_jobs.write().await.remove(id);
    }

    Ok(VideoGenerationResponse {
        id: id.to_string(),
        status: poll.status,
        model: handle.model.clone(),
        provider: handle.provider.clone(),
        video_url: poll.video_url,
        progress: poll.progress,
        error: poll.error,
        usage: None,
        created: chrono::Utc::now().timestamp(),
    })
}
```

If `ProxyError` does not have a `NotFound` variant, add it. Search `router.rs` (or wherever `ProxyError` is defined) for the enum and append:
```rust
#[error("not found: {0}")]
NotFound(String),
```

If `e.status_code()` is implemented for `ProxyError`, add a 404 case for `NotFound` returning `404`.

- [ ] **Step 4: Add the handler**

In [litegen-core/src/api/handlers.rs](litegen-core/src/api/handlers.rs), append (near the other video handlers):

```rust
/// GET /v1/videos/{id} — Poll the status of a video generation.
#[utoipa::path(
    get,
    path = "/v1/videos/{id}",
    params(("id" = String, Path, description = "Video generation ID")),
    responses(
        (status = 200, description = "Current status", body = VideoGenerationResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Videos"
)]
pub async fn get_video_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.router.get_video_status(&id).await {
        Ok(resp) => (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}
```

In `create_router`, add the route:
```rust
.route("/v1/videos/{id}", get(get_video_status))
```

(Place it next to the other video routes.)

- [ ] **Step 5: Write a router test**

Append to [litegen-core/src/proxy/router_tests.rs](litegen-core/src/proxy/router_tests.rs):

```rust
#[tokio::test]
async fn get_video_status_returns_not_found_for_unknown_id() {
    let router = test_router().await; // use existing test helper if present; otherwise build one
    let result = router.get_video_status("nonexistent-id").await;
    assert!(result.is_err());
}
```

If no `test_router()` helper exists, mimic the setup used by adjacent tests in the file.

- [ ] **Step 6: Run tests**

Run: `cargo test -p litegen-core --test router_tests` (or whatever the test invocation already used in this file is).
Expected: New test passes; existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/proxy/router.rs litegen-core/src/api/handlers.rs litegen-core/src/proxy/router_tests.rs
git commit -m "feat(litegen-core): add GET /v1/videos/{id} status endpoint"
```

---

### Task 4: Add OpenAPI aggregator and `/openapi.json` route

**Why:** This is what the SDKs consume.

**Files:**
- Create: `litegen-core/src/api/openapi.rs`
- Modify: `litegen-core/src/api/mod.rs`
- Modify: `litegen-core/src/api/handlers.rs` (add `/openapi.json` route)

- [ ] **Step 1: Create the aggregator**

Create [litegen-core/src/api/openapi.rs](litegen-core/src/api/openapi.rs):

```rust
use utoipa::OpenApi;

use crate::api::handlers;
use crate::capabilities;
use crate::types;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "LiteGen API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Universal proxy for AI image and video generation."
    ),
    paths(
        handlers::generate_image,
        handlers::estimate_image_cost,
        handlers::generate_video,
        handlers::estimate_video_cost,
        handlers::get_video_status,
        handlers::list_models,
        handlers::get_model_schema,
        handlers::health_check,
        handlers::liveness,
        handlers::get_stats,
        handlers::get_logs,
        handlers::create_api_key,
        handlers::list_api_keys,
        handlers::revoke_api_key,
        handlers::clear_cache,
    ),
    components(schemas(
        // Request / response types
        types::BaseGenerationRequest,
        types::ImageGenerationRequest,
        types::ImageGenerationResponse,
        types::ImageResult,
        types::VideoGenerationRequest,
        types::VideoGenerationResponse,
        types::ReferenceImage,
        types::RefImageKind,
        types::GenerationStatus,
        types::MediaType,
        types::CostSource,
        types::UsageInfo,
        types::CostEstimate,
        types::ModelInfo,
        types::ModelCapabilities,
        types::ModelPricing,
        types::ProviderConfig,
        types::ApiKeyEntry,
        types::ModelRoute,
        types::Deployment,
        types::RoutingStrategy,
        types::CacheConfig,
        types::ProviderHealth,
        types::RequestLog,
        types::ProxyStats,
        types::ModelUsageStat,
        types::ProviderUsageStat,
        types::ErrorResponse,
        types::ErrorDetail,
        types::ApiKey,
        types::LivenessResponse,
        types::CacheStatus,
        types::HealthResponse,
        types::ModelListResponse,
        types::ApiKeyInfo,
        types::ApiKeyListResponse,
        types::ApiKeyCreatedResponse,
        types::RevokeKeyResponse,
        types::CacheClearedResponse,
        // Capability schema (response of GET /v1/models/{id})
        capabilities::ModelSchema,
        capabilities::ModelCapabilityFlags,
        capabilities::ModelPricing as CapabilityModelPricing,
        capabilities::PromptSpec,
        capabilities::ParamSpec,
        capabilities::SizeSpec,
        capabilities::RefInputSpec,
        capabilities::RefRoleSpec,
        capabilities::RefProviderFormat,
        capabilities::MediaType as CapabilityMediaType,
    )),
    tags(
        (name = "Images", description = "Image generation endpoints"),
        (name = "Videos", description = "Video generation endpoints"),
        (name = "Models", description = "Model discovery"),
        (name = "System", description = "Health checks"),
        (name = "Dashboard", description = "Stats and logs"),
        (name = "Admin", description = "API key management"),
    )
)]
pub struct ApiDoc;
```

Note: if any of the type names above don't actually re-export from `capabilities::` (because they're not `pub use`'d in `mod.rs`), check `capabilities/mod.rs` and add re-exports as needed. The schema types live in `capabilities/schema.rs`.

- [ ] **Step 2: Register the module**

In [litegen-core/src/api/mod.rs](litegen-core/src/api/mod.rs):

```rust
pub mod handlers;
pub mod middleware;
pub mod openapi;

pub use handlers::create_router;
```

- [ ] **Step 3: Add the `/openapi.json` route**

In [litegen-core/src/api/handlers.rs](litegen-core/src/api/handlers.rs), add a handler:

```rust
/// GET /openapi.json — OpenAPI specification.
pub async fn openapi_spec() -> impl IntoResponse {
    use utoipa::OpenApi;
    Json(crate::api::openapi::ApiDoc::openapi())
}
```

In `create_router`, add the route:
```rust
.route("/openapi.json", get(openapi_spec))
```

- [ ] **Step 4: Build**

Run: `cargo build -p litegen-core`
Expected: Success. If any `ToSchema` is missing on a referenced type, the build fails — add `ToSchema` to that type and retry.

- [ ] **Step 5: Spot-check the spec**

Run litegen-core: `LITEGEN_MODELS_DIR=models cargo run -p litegen-core &`
Wait ~3s, then: `curl -s http://localhost:4000/openapi.json | head -50`
Expected: Valid OpenAPI 3.x JSON with `info.title`, `paths`, `components.schemas`.
Kill the server: `kill %1`.

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/api/openapi.rs litegen-core/src/api/mod.rs litegen-core/src/api/handlers.rs
git commit -m "feat(litegen-core): expose OpenAPI spec at /openapi.json"
```

---

## Phase 2 — SDK directory and codegen scaffolding

### Task 5: Scaffold `sdks/` directory and codegen scripts

**Files:**
- Create: `sdks/README.md`
- Create: `sdks/scripts/fetch-openapi.sh`
- Create: `sdks/scripts/regen-all.sh`
- Create: `.gitignore` entries if needed

- [ ] **Step 1: Create `sdks/README.md`**

```markdown
# LiteGen SDKs

First-party SDKs for the LiteGen HTTP API, generated from the OpenAPI spec
served by `litegen-core` at `/openapi.json`.

## Packages

- `typescript/` — `@litegen/sdk` (npm). Universal (Node 18+, browsers, Deno, Bun).
- `python/` — `litegen` (PyPI). Python 3.10+, sync + async clients.

## Regenerating

```bash
# Boots litegen-core, fetches /openapi.json, regenerates both SDKs.
./scripts/regen-all.sh
```

If litegen-core is already running:
```bash
LITEGEN_BASE_URL=http://localhost:4000 ./scripts/regen-all.sh
```

CI fails if `git diff sdks/` reports changes after running this script.
```

- [ ] **Step 2: Create `sdks/scripts/fetch-openapi.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUTPUT="${REPO_ROOT}/sdks/openapi.json"

if [[ -n "${LITEGEN_BASE_URL:-}" ]]; then
  echo "Using running litegen-core at ${LITEGEN_BASE_URL}"
  curl -sf "${LITEGEN_BASE_URL}/openapi.json" \
    | python3 -m json.tool > "${OUTPUT}"
  echo "Wrote ${OUTPUT}"
  exit 0
fi

echo "Starting litegen-core for codegen..."
cd "${REPO_ROOT}/litegen-core"
LITEGEN_MODELS_DIR="${REPO_ROOT}/models" cargo run --release --quiet &
SERVER_PID=$!
trap "kill ${SERVER_PID} 2>/dev/null || true" EXIT

# Wait for liveness
for i in {1..30}; do
  if curl -sf http://localhost:4000/health/live >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

curl -sf http://localhost:4000/openapi.json \
  | python3 -m json.tool > "${OUTPUT}"
echo "Wrote ${OUTPUT}"
```

Make it executable: `chmod +x sdks/scripts/fetch-openapi.sh`

- [ ] **Step 3: Create `sdks/scripts/regen-all.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SDKS_DIR="${REPO_ROOT}/sdks"

echo "==> Fetching OpenAPI spec"
"${SCRIPT_DIR}/fetch-openapi.sh"

echo "==> Regenerating TypeScript SDK"
cd "${SDKS_DIR}/typescript"
npm install --silent --no-audit --no-fund
npx --yes openapi-typescript "${SDKS_DIR}/openapi.json" \
  -o "${SDKS_DIR}/typescript/src/generated/schema.d.ts"

echo "==> Regenerating Python SDK"
cd "${SDKS_DIR}/python"
# Use pipx if available, else pip install --user
if command -v pipx >/dev/null 2>&1; then
  pipx run openapi-python-client generate \
    --path "${SDKS_DIR}/openapi.json" \
    --config "${SDKS_DIR}/python/codegen.yml" \
    --overwrite \
    --output-path "${SDKS_DIR}/python/litegen/_generated"
else
  python3 -m pip install --quiet --user openapi-python-client
  python3 -m openapi_python_client generate \
    --path "${SDKS_DIR}/openapi.json" \
    --config "${SDKS_DIR}/python/codegen.yml" \
    --overwrite \
    --output-path "${SDKS_DIR}/python/litegen/_generated"
fi

echo "==> Done. Review and commit changes under sdks/."
```

Make executable: `chmod +x sdks/scripts/regen-all.sh`

- [ ] **Step 4: Commit**

```bash
git add sdks/README.md sdks/scripts/
git commit -m "feat(sdks): scaffold codegen scripts and README"
```

---

### Task 6: Generate the `openapi.json` snapshot

**Files:**
- Create: `sdks/openapi.json`

- [ ] **Step 1: Run the fetch script**

```bash
cd /Users/joeviscardi/source/repos/litegen
./sdks/scripts/fetch-openapi.sh
```

Expected: `sdks/openapi.json` created, pretty-printed, ~50-200kb.

If litegen-core fails to start due to missing provider keys, set up a minimal model fixture or use a stubbed config. In a pinch, run with `LITEGEN_MODELS_DIR=` pointing at an empty dir — the openapi endpoint should still work since it doesn't depend on models being loaded successfully (only on the router starting). If the server refuses to start, debug minimally and ensure `/openapi.json` works.

- [ ] **Step 2: Sanity check**

```bash
python3 -c "import json; spec = json.load(open('sdks/openapi.json')); print(len(spec['paths']), 'paths,', len(spec['components']['schemas']), 'schemas')"
```

Expected: At least 15 paths and 30+ schemas.

- [ ] **Step 3: Commit**

```bash
git add sdks/openapi.json
git commit -m "feat(sdks): commit initial OpenAPI snapshot"
```

---

## Phase 3 — TypeScript SDK

### Task 7: Initialize TypeScript SDK package

**Files:**
- Create: `sdks/typescript/package.json`
- Create: `sdks/typescript/tsconfig.json`
- Create: `sdks/typescript/tsup.config.ts`
- Create: `sdks/typescript/.gitignore`
- Create: `sdks/typescript/src/index.ts` (placeholder)

- [ ] **Step 1: `package.json`**

```json
{
  "name": "@litegen/sdk",
  "version": "0.1.0",
  "description": "First-party TypeScript SDK for LiteGen — universal AI image & video generation proxy.",
  "license": "MIT",
  "type": "module",
  "main": "./dist/index.cjs",
  "module": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".": {
      "import": {
        "types": "./dist/index.d.ts",
        "default": "./dist/index.js"
      },
      "require": {
        "types": "./dist/index.d.cts",
        "default": "./dist/index.cjs"
      }
    }
  },
  "files": ["dist", "README.md"],
  "scripts": {
    "build": "tsup",
    "typecheck": "tsc --noEmit",
    "test": "vitest run",
    "test:watch": "vitest",
    "gen": "../scripts/regen-all.sh"
  },
  "engines": { "node": ">=18" },
  "devDependencies": {
    "@types/node": "^20.0.0",
    "openapi-typescript": "^7.0.0",
    "tsup": "^8.0.0",
    "typescript": "^5.4.0",
    "vitest": "^1.6.0"
  }
}
```

- [ ] **Step 2: `tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "lib": ["ES2022", "DOM"],
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "declaration": true,
    "outDir": "./dist",
    "rootDir": "./src",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "forceConsistentCasingInFileNames": true
  },
  "include": ["src/**/*", "test/**/*"]
}
```

- [ ] **Step 3: `tsup.config.ts`**

```ts
import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm", "cjs"],
  dts: true,
  sourcemap: true,
  clean: true,
  target: "es2022",
  splitting: false,
  treeshake: true,
});
```

- [ ] **Step 4: `.gitignore`**

```
dist/
node_modules/
*.tsbuildinfo
```

- [ ] **Step 5: Placeholder `src/index.ts`**

```ts
export {};
```

- [ ] **Step 6: Install dependencies**

```bash
cd sdks/typescript && npm install
```

Expected: Lockfile created, `node_modules/` populated.

- [ ] **Step 7: Commit**

```bash
git add sdks/typescript/package.json sdks/typescript/tsconfig.json sdks/typescript/tsup.config.ts sdks/typescript/.gitignore sdks/typescript/src/index.ts sdks/typescript/package-lock.json
git commit -m "feat(sdks/ts): initialize package scaffold"
```

---

### Task 8: Generate TypeScript types from OpenAPI

**Files:**
- Create: `sdks/typescript/src/generated/schema.d.ts`

- [ ] **Step 1: Run codegen**

```bash
cd sdks/typescript
npx openapi-typescript ../openapi.json -o src/generated/schema.d.ts
```

Expected: `src/generated/schema.d.ts` created (~10-50kb of types).

- [ ] **Step 2: Verify it compiles**

```bash
cd sdks/typescript && npx tsc --noEmit
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add sdks/typescript/src/generated/schema.d.ts
git commit -m "feat(sdks/ts): generate types from OpenAPI spec"
```

---

### Task 9: TypeScript error classes

**Files:**
- Create: `sdks/typescript/src/errors.ts`

- [ ] **Step 1: Write `errors.ts`**

```ts
import type { components } from "./generated/schema";

type ErrorDetail = components["schemas"]["ErrorDetail"];

export class LiteGenError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LiteGenError";
  }
}

export class LiteGenAPIError extends LiteGenError {
  readonly status: number;
  readonly type: string;
  readonly code?: string;
  readonly providerError?: unknown;

  constructor(status: number, detail: ErrorDetail) {
    super(detail.message);
    this.name = "LiteGenAPIError";
    this.status = status;
    this.type = detail.type;
    this.code = detail.code ?? undefined;
    this.providerError = detail.provider_error ?? undefined;
  }
}

export class LiteGenTimeoutError extends LiteGenError {
  constructor(message = "LiteGen request timed out") {
    super(message);
    this.name = "LiteGenTimeoutError";
  }
}

export class LiteGenPollingTimeoutError extends LiteGenError {
  readonly lastStatus?: string;
  constructor(id: string, lastStatus?: string) {
    super(`Polling for video '${id}' timed out${lastStatus ? ` (last status: ${lastStatus})` : ""}`);
    this.name = "LiteGenPollingTimeoutError";
    this.lastStatus = lastStatus;
  }
}
```

- [ ] **Step 2: Typecheck**

```bash
cd sdks/typescript && npx tsc --noEmit
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add sdks/typescript/src/errors.ts
git commit -m "feat(sdks/ts): add typed error classes"
```

---

### Task 10: TypeScript polling helper

**Files:**
- Create: `sdks/typescript/src/polling.ts`

- [ ] **Step 1: Write `polling.ts`**

```ts
import type { components } from "./generated/schema";
import { LiteGenPollingTimeoutError } from "./errors";

type VideoResponse = components["schemas"]["VideoGenerationResponse"];

export interface WaitForCompletionOptions {
  /** Milliseconds between polls. Default 2000. */
  intervalMs?: number;
  /** Total timeout in milliseconds. Default 5 minutes. */
  timeoutMs?: number;
  /** Optional AbortSignal to cancel polling. */
  signal?: AbortSignal;
}

const TERMINAL_STATUSES = new Set(["completed", "failed", "cancelled"]);

export async function waitForCompletion(
  id: string,
  getStatus: (id: string) => Promise<VideoResponse>,
  opts: WaitForCompletionOptions = {},
): Promise<VideoResponse> {
  const intervalMs = opts.intervalMs ?? 2000;
  const timeoutMs = opts.timeoutMs ?? 5 * 60_000;
  const deadline = Date.now() + timeoutMs;

  let last: VideoResponse | undefined;
  while (true) {
    if (opts.signal?.aborted) {
      throw new DOMException("Polling aborted", "AbortError");
    }
    if (Date.now() > deadline) {
      throw new LiteGenPollingTimeoutError(id, last?.status);
    }
    last = await getStatus(id);
    if (TERMINAL_STATUSES.has(last.status as string)) {
      return last;
    }
    await sleep(intervalMs, opts.signal);
  }
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => resolve(), ms);
    signal?.addEventListener("abort", () => {
      clearTimeout(t);
      reject(new DOMException("Polling aborted", "AbortError"));
    }, { once: true });
  });
}
```

- [ ] **Step 2: Typecheck**

```bash
cd sdks/typescript && npx tsc --noEmit
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add sdks/typescript/src/polling.ts
git commit -m "feat(sdks/ts): add waitForCompletion polling helper"
```

---

### Task 11: TypeScript main client

**Files:**
- Create: `sdks/typescript/src/client.ts`

- [ ] **Step 1: Write `client.ts`**

```ts
import type { components } from "./generated/schema";
import { LiteGenAPIError, LiteGenTimeoutError } from "./errors";
import { waitForCompletion, type WaitForCompletionOptions } from "./polling";

type Schemas = components["schemas"];
type ImageRequest = Schemas["ImageGenerationRequest"];
type ImageResponse = Schemas["ImageGenerationResponse"];
type VideoRequest = Schemas["VideoGenerationRequest"];
type VideoResponse = Schemas["VideoGenerationResponse"];
type CostEstimate = Schemas["CostEstimate"];
type ModelInfo = Schemas["ModelInfo"];
type ModelSchema = Schemas["ModelSchema"];
type ModelListResponse = Schemas["ModelListResponse"];
type HealthResponse = Schemas["HealthResponse"];
type LivenessResponse = Schemas["LivenessResponse"];
type ProxyStats = Schemas["ProxyStats"];
type RequestLog = Schemas["RequestLog"];
type PaginatedLogs = { data: RequestLog[]; total: number; page: number; per_page: number; total_pages: number };
type ApiKeyInfo = Schemas["ApiKeyInfo"];
type ApiKeyListResponse = Schemas["ApiKeyListResponse"];
type ApiKeyCreatedResponse = Schemas["ApiKeyCreatedResponse"];
type RevokeKeyResponse = Schemas["RevokeKeyResponse"];
type CacheClearedResponse = Schemas["CacheClearedResponse"];

export type FetchLike = typeof fetch;

export interface LiteGenClientOptions {
  baseUrl?: string;
  apiKey?: string;
  fetch?: FetchLike;
  timeoutMs?: number;
  defaultHeaders?: Record<string, string>;
}

export class LiteGenClient {
  private readonly baseUrl: string;
  private readonly apiKey?: string;
  private readonly fetchImpl: FetchLike;
  private readonly timeoutMs: number;
  private readonly defaultHeaders: Record<string, string>;

  readonly images: ImagesNamespace;
  readonly videos: VideosNamespace;
  readonly models: ModelsNamespace;
  readonly health: HealthNamespace;
  readonly stats: StatsNamespace;
  readonly logs: LogsNamespace;
  readonly keys: KeysNamespace;
  readonly cache: CacheNamespace;

  constructor(opts: LiteGenClientOptions = {}) {
    this.baseUrl = (opts.baseUrl ?? "http://localhost:4000").replace(/\/$/, "");
    this.apiKey = opts.apiKey;
    this.fetchImpl = opts.fetch ?? globalThis.fetch.bind(globalThis);
    this.timeoutMs = opts.timeoutMs ?? 60_000;
    this.defaultHeaders = opts.defaultHeaders ?? {};

    this.images = new ImagesNamespace(this);
    this.videos = new VideosNamespace(this);
    this.models = new ModelsNamespace(this);
    this.health = new HealthNamespace(this);
    this.stats = new StatsNamespace(this);
    this.logs = new LogsNamespace(this);
    this.keys = new KeysNamespace(this);
    this.cache = new CacheNamespace(this);
  }

  /** @internal */
  async request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...this.defaultHeaders,
    };
    if (this.apiKey) headers["Authorization"] = `Bearer ${this.apiKey}`;

    const ctrl = new AbortController();
    const timeout = setTimeout(() => ctrl.abort(), this.timeoutMs);
    if (signal) {
      signal.addEventListener("abort", () => ctrl.abort(), { once: true });
    }

    let res: Response;
    try {
      res = await this.fetchImpl(url, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
        signal: ctrl.signal,
      });
    } catch (err) {
      if ((err as { name?: string }).name === "AbortError" && !signal?.aborted) {
        throw new LiteGenTimeoutError();
      }
      throw err;
    } finally {
      clearTimeout(timeout);
    }

    if (!res.ok) {
      let detail = { message: res.statusText, type: "http_error", code: String(res.status), provider_error: undefined };
      try {
        const parsed = await res.json();
        if (parsed?.error) detail = parsed.error;
      } catch {}
      throw new LiteGenAPIError(res.status, detail as Schemas["ErrorDetail"]);
    }
    return (await res.json()) as T;
  }
}

class ImagesNamespace {
  constructor(private readonly client: LiteGenClient) {}
  generate(req: ImageRequest, signal?: AbortSignal): Promise<ImageResponse> {
    return this.client.request("POST", "/v1/images/generations", req, signal);
  }
  estimateCost(req: ImageRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/images/cost", req, signal);
  }
}

class VideosNamespace {
  constructor(private readonly client: LiteGenClient) {}
  generate(req: VideoRequest, signal?: AbortSignal): Promise<VideoResponse> {
    return this.client.request("POST", "/v1/videos/generations", req, signal);
  }
  estimateCost(req: VideoRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/videos/cost", req, signal);
  }
  getStatus(id: string, signal?: AbortSignal): Promise<VideoResponse> {
    return this.client.request("GET", `/v1/videos/${encodeURIComponent(id)}`, undefined, signal);
  }
  waitForCompletion(id: string, opts?: WaitForCompletionOptions): Promise<VideoResponse> {
    return waitForCompletion(id, (videoId) => this.getStatus(videoId, opts?.signal), opts);
  }
}

class ModelsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  async list(signal?: AbortSignal): Promise<ModelInfo[]> {
    const resp = await this.client.request<ModelListResponse>("GET", "/v1/models", undefined, signal);
    return resp.data;
  }
  get(id: string, signal?: AbortSignal): Promise<ModelSchema> {
    return this.client.request("GET", `/v1/models/${encodeURIComponent(id)}`, undefined, signal);
  }
}

class HealthNamespace {
  constructor(private readonly client: LiteGenClient) {}
  check(signal?: AbortSignal): Promise<HealthResponse> {
    return this.client.request("GET", "/health", undefined, signal);
  }
  live(signal?: AbortSignal): Promise<LivenessResponse> {
    return this.client.request("GET", "/health/live", undefined, signal);
  }
}

class StatsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(signal?: AbortSignal): Promise<ProxyStats> {
    return this.client.request("GET", "/v1/stats", undefined, signal);
  }
}

class LogsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  list(opts: { page?: number; perPage?: number } = {}, signal?: AbortSignal): Promise<PaginatedLogs> {
    const page = opts.page ?? 1;
    const perPage = opts.perPage ?? 50;
    return this.client.request("GET", `/v1/logs?page=${page}&per_page=${perPage}`, undefined, signal);
  }
}

class KeysNamespace {
  constructor(private readonly client: LiteGenClient) {}
  create(name: string, signal?: AbortSignal): Promise<ApiKeyCreatedResponse> {
    return this.client.request("POST", "/v1/keys", { name }, signal);
  }
  async list(signal?: AbortSignal): Promise<ApiKeyInfo[]> {
    const resp = await this.client.request<ApiKeyListResponse>("GET", "/v1/keys", undefined, signal);
    return resp.data;
  }
  revoke(id: string, signal?: AbortSignal): Promise<RevokeKeyResponse> {
    return this.client.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`, undefined, signal);
  }
}

class CacheNamespace {
  constructor(private readonly client: LiteGenClient) {}
  clear(signal?: AbortSignal): Promise<CacheClearedResponse> {
    return this.client.request("DELETE", "/v1/cache", undefined, signal);
  }
}
```

- [ ] **Step 2: Typecheck**

```bash
cd sdks/typescript && npx tsc --noEmit
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add sdks/typescript/src/client.ts
git commit -m "feat(sdks/ts): implement LiteGenClient with namespaced methods"
```

---

### Task 12: TypeScript public exports

**Files:**
- Modify: `sdks/typescript/src/index.ts`

- [ ] **Step 1: Replace placeholder index**

```ts
export { LiteGenClient, type LiteGenClientOptions, type FetchLike } from "./client";
export {
  LiteGenError,
  LiteGenAPIError,
  LiteGenTimeoutError,
  LiteGenPollingTimeoutError,
} from "./errors";
export { waitForCompletion, type WaitForCompletionOptions } from "./polling";

// Re-export request/response types and enums.
export type { components, operations, paths } from "./generated/schema";

// Convenience enum-like constants (the API returns the snake_case strings).
export const GenerationStatus = {
  Pending: "pending",
  Processing: "processing",
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
} as const;
export type GenerationStatus = typeof GenerationStatus[keyof typeof GenerationStatus];

export const MediaType = {
  Image: "image",
  Video: "video",
} as const;
export type MediaType = typeof MediaType[keyof typeof MediaType];

export const CostSource = {
  Dynamic: "dynamic",
  Estimated: "estimated",
} as const;
export type CostSource = typeof CostSource[keyof typeof CostSource];

export const RoutingStrategy = {
  Fallback: "fallback",
  WeightedRoundRobin: "weighted_round_robin",
  LowestCost: "lowest_cost",
  LowestLatency: "lowest_latency",
} as const;
export type RoutingStrategy = typeof RoutingStrategy[keyof typeof RoutingStrategy];

export const RefImageKind = {
  Base64: "base64",
  Url: "url",
  Blob: "blob",
} as const;
export type RefImageKind = typeof RefImageKind[keyof typeof RefImageKind];

// Convenience type aliases for the most common payloads.
import type { components } from "./generated/schema";
export type ImageGenerationRequest = components["schemas"]["ImageGenerationRequest"];
export type ImageGenerationResponse = components["schemas"]["ImageGenerationResponse"];
export type ImageResult = components["schemas"]["ImageResult"];
export type VideoGenerationRequest = components["schemas"]["VideoGenerationRequest"];
export type VideoGenerationResponse = components["schemas"]["VideoGenerationResponse"];
export type ReferenceImage = components["schemas"]["ReferenceImage"];
export type ModelInfo = components["schemas"]["ModelInfo"];
export type ModelSchema = components["schemas"]["ModelSchema"];
export type CostEstimate = components["schemas"]["CostEstimate"];
export type UsageInfo = components["schemas"]["UsageInfo"];
export type ProxyStats = components["schemas"]["ProxyStats"];
export type RequestLog = components["schemas"]["RequestLog"];
export type ProviderHealth = components["schemas"]["ProviderHealth"];
export type ApiKeyInfo = components["schemas"]["ApiKeyInfo"];
```

- [ ] **Step 2: Build the package**

```bash
cd sdks/typescript && npm run build
```

Expected: `dist/index.js`, `dist/index.cjs`, `dist/index.d.ts`, `dist/index.d.cts` all created.

- [ ] **Step 3: Commit**

```bash
git add sdks/typescript/src/index.ts
git commit -m "feat(sdks/ts): wire up public exports and enum constants"
```

---

### Task 13: TypeScript unit tests

**Files:**
- Create: `sdks/typescript/test/client.test.ts`

- [ ] **Step 1: Write tests**

```ts
import { describe, it, expect, vi } from "vitest";
import { LiteGenClient, LiteGenAPIError, GenerationStatus, RefImageKind } from "../src";

function mockFetch(handler: (req: { url: string; method: string; headers: Record<string, string>; body: unknown }) => Response | Promise<Response>) {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const headers: Record<string, string> = {};
    const h = new Headers(init?.headers);
    h.forEach((v, k) => { headers[k] = v; });
    const body = init?.body ? JSON.parse(init.body as string) : undefined;
    return handler({ url, method: init?.method ?? "GET", headers, body });
  }) as unknown as typeof fetch;
}

describe("LiteGenClient.images.generate", () => {
  it("posts to /v1/images/generations with auth header and body", async () => {
    const captured: any = {};
    const fetchImpl = mockFetch(async (req) => {
      captured.url = req.url;
      captured.method = req.method;
      captured.headers = req.headers;
      captured.body = req.body;
      return new Response(JSON.stringify({
        created: 1, data: [], model: "openai/dall-e-3", provider: "openai", id: "img-1"
      }), { status: 200, headers: { "Content-Type": "application/json" } });
    });
    const client = new LiteGenClient({ apiKey: "lg-test", fetch: fetchImpl });
    const resp = await client.images.generate({
      prompt: "a cat", model: "openai/dall-e-3", n: 1, strict: true,
      reference_images: [], response_format: "url",
    } as any);
    expect(captured.url).toContain("/v1/images/generations");
    expect(captured.method).toBe("POST");
    expect(captured.headers["authorization"]).toBe("Bearer lg-test");
    expect(captured.body.prompt).toBe("a cat");
    expect(resp.id).toBe("img-1");
  });

  it("throws LiteGenAPIError on non-2xx", async () => {
    const fetchImpl = mockFetch(async () =>
      new Response(JSON.stringify({ error: { message: "bad prompt", type: "validation_error", code: "400" } }),
        { status: 400, headers: { "Content-Type": "application/json" } })
    );
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    await expect(client.images.generate({ prompt: "", model: "x" } as any))
      .rejects.toBeInstanceOf(LiteGenAPIError);
  });
});

describe("LiteGenClient.videos.waitForCompletion", () => {
  it("polls until status is completed", async () => {
    let calls = 0;
    const fetchImpl = mockFetch(async (req) => {
      if (req.url.endsWith("/v1/videos/vid-1")) {
        calls++;
        const status = calls < 3 ? "processing" : "completed";
        return new Response(JSON.stringify({
          id: "vid-1", status, model: "m", provider: "p", progress: status === "completed" ? 100 : 50, created: 1,
        }), { status: 200, headers: { "Content-Type": "application/json" } });
      }
      return new Response("nope", { status: 404 });
    });
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    const final = await client.videos.waitForCompletion("vid-1", { intervalMs: 10, timeoutMs: 2000 });
    expect(final.status).toBe(GenerationStatus.Completed);
    expect(calls).toBe(3);
  });
});

describe("enum constants", () => {
  it("matches API string values", () => {
    expect(GenerationStatus.Completed).toBe("completed");
    expect(RefImageKind.Url).toBe("url");
  });
});

describe("LiteGenClient.models.list", () => {
  it("unwraps the {object,data} envelope", async () => {
    const fetchImpl = mockFetch(async () =>
      new Response(JSON.stringify({ object: "list", data: [{ id: "m1" }] }),
        { status: 200, headers: { "Content-Type": "application/json" } })
    );
    const client = new LiteGenClient({ fetch: fetchImpl });
    const models = await client.models.list();
    expect(models).toHaveLength(1);
    expect((models[0] as any).id).toBe("m1");
  });
});
```

- [ ] **Step 2: Run tests**

```bash
cd sdks/typescript && npm test
```

Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add sdks/typescript/test/client.test.ts
git commit -m "test(sdks/ts): unit tests for client, polling, enums"
```

---

### Task 14: TypeScript examples

**Files:**
- Create: `sdks/typescript/examples/generate-image.ts`
- Create: `sdks/typescript/examples/generate-video.ts`

- [ ] **Step 1: `examples/generate-image.ts`**

```ts
import { LiteGenClient } from "../src";

const client = new LiteGenClient({
  baseUrl: process.env.LITEGEN_BASE_URL ?? "http://localhost:4000",
  apiKey: process.env.LITEGEN_API_KEY,
});

const result = await client.images.generate({
  prompt: "a serene mountain landscape at sunset, oil painting",
  model: "openai/dall-e-3",
  size: "1024x1024",
  quality: "hd",
  n: 1,
  strict: true,
  reference_images: [],
  response_format: "url",
});

console.log("Image URL:", result.data[0]?.url);
console.log("Provider:", result.provider);
console.log("Cost USD:", result.usage?.cost_usd);
```

- [ ] **Step 2: `examples/generate-video.ts`**

```ts
import { LiteGenClient, GenerationStatus } from "../src";

const client = new LiteGenClient({
  baseUrl: process.env.LITEGEN_BASE_URL ?? "http://localhost:4000",
  apiKey: process.env.LITEGEN_API_KEY,
});

const job = await client.videos.generate({
  prompt: "a timelapse of clouds drifting over a quiet city",
  model: "runway/gen-3",
  duration_seconds: 5,
  n: 1,
  strict: true,
  reference_images: [],
});

console.log("Job started:", job.id);

const final = await client.videos.waitForCompletion(job.id, {
  intervalMs: 5_000,
  timeoutMs: 10 * 60_000,
});

if (final.status === GenerationStatus.Completed) {
  console.log("Video URL:", final.video_url);
} else {
  console.error("Generation failed:", final.error);
}
```

- [ ] **Step 3: Typecheck examples**

Append to `sdks/typescript/tsconfig.json`'s `include`:
```json
  "include": ["src/**/*", "test/**/*", "examples/**/*"]
```

Then:
```bash
cd sdks/typescript && npx tsc --noEmit
```

Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add sdks/typescript/examples/ sdks/typescript/tsconfig.json
git commit -m "docs(sdks/ts): add runnable image and video examples"
```

---

## Phase 4 — Python SDK

### Task 15: Initialize Python SDK package

**Files:**
- Create: `sdks/python/pyproject.toml`
- Create: `sdks/python/codegen.yml`
- Create: `sdks/python/.gitignore`
- Create: `sdks/python/litegen/__init__.py` (placeholder)

- [ ] **Step 1: `pyproject.toml`**

```toml
[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[project]
name = "litegen"
version = "0.1.0"
description = "First-party Python SDK for LiteGen — universal AI image & video generation proxy."
readme = "README.md"
requires-python = ">=3.10"
license = { text = "MIT" }
authors = [{ name = "LiteGen" }]
dependencies = [
    "httpx>=0.27",
    "pydantic>=2.5",
    "attrs>=23.0",
    "python-dateutil>=2.8",
]

[project.optional-dependencies]
dev = [
    "pytest>=8.0",
    "pytest-asyncio>=0.23",
    "respx>=0.21",
    "mypy>=1.10",
]

[tool.hatch.build.targets.wheel]
packages = ["litegen"]

[tool.pytest.ini_options]
asyncio_mode = "auto"
```

- [ ] **Step 2: `codegen.yml`**

```yaml
package_name_override: _generated
project_name_override: _generated
use_path_prefixes_for_title_model_names: false
field_constraints: true
post_hooks: []
```

- [ ] **Step 3: `.gitignore`**

```
__pycache__/
*.egg-info/
.pytest_cache/
.mypy_cache/
.venv/
build/
dist/
```

- [ ] **Step 4: Placeholder `litegen/__init__.py`**

```python
__version__ = "0.1.0"
```

- [ ] **Step 5: Create `litegen/_generated/` placeholder**

```bash
mkdir -p sdks/python/litegen/_generated
touch sdks/python/litegen/_generated/__init__.py
```

- [ ] **Step 6: Commit**

```bash
git add sdks/python/pyproject.toml sdks/python/codegen.yml sdks/python/.gitignore sdks/python/litegen/__init__.py sdks/python/litegen/_generated/__init__.py
git commit -m "feat(sdks/python): initialize package scaffold"
```

---

### Task 16: Generate Python client from OpenAPI

**Files:**
- Generated: `sdks/python/litegen/_generated/**`

- [ ] **Step 1: Run openapi-python-client**

```bash
cd sdks/python
python3 -m pip install --quiet --user openapi-python-client
# openapi-python-client generates a *package* — we want its contents flattened into _generated/.
rm -rf litegen/_generated
python3 -m openapi_python_client generate \
  --path ../openapi.json \
  --config codegen.yml \
  --overwrite \
  --output-path .
# The tool creates a directory named after the package (`_generated/`). Move its contents.
if [ -d "_generated" ]; then
  rm -rf litegen/_generated
  mv _generated litegen/_generated
fi
```

Expected: `sdks/python/litegen/_generated/` populated with `models/`, `api/`, `client.py`, `errors.py`, `types.py`, `__init__.py`.

If `openapi-python-client` produces a different directory layout than expected, adjust the move step accordingly. The goal: `litegen._generated.models.*`, `litegen._generated.api.*`, `litegen._generated.client.*` are importable.

- [ ] **Step 2: Smoke-test imports**

```bash
cd sdks/python
python3 -c "from litegen._generated.models import ImageGenerationRequest; print(ImageGenerationRequest.__fields__.keys())"
```

Expected: Prints the field names of the request model.

- [ ] **Step 3: Commit**

```bash
git add sdks/python/litegen/_generated
git commit -m "feat(sdks/python): generate pydantic models and http client from OpenAPI"
```

---

### Task 17: Python error classes

**Files:**
- Create: `sdks/python/litegen/errors.py`

- [ ] **Step 1: Write `errors.py`**

```python
"""Public error types raised by the LiteGen SDK."""
from __future__ import annotations

from typing import Any


class LiteGenError(Exception):
    """Base class for all SDK errors."""


class LiteGenAPIError(LiteGenError):
    """Raised when the API returns a non-2xx response."""

    def __init__(
        self,
        status: int,
        message: str,
        *,
        type: str = "api_error",
        code: str | None = None,
        provider_error: Any = None,
    ) -> None:
        super().__init__(message)
        self.status = status
        self.type = type
        self.code = code
        self.provider_error = provider_error


class LiteGenValidationError(LiteGenAPIError):
    """Raised on 400 responses with `type == "validation_error"`."""


class LiteGenTimeoutError(LiteGenError):
    """Raised when a request times out at the transport layer."""


class LiteGenPollingTimeoutError(LiteGenError):
    """Raised when `wait_for_video_completion` exceeds its timeout."""

    def __init__(self, video_id: str, last_status: str | None = None) -> None:
        super().__init__(
            f"Polling for video '{video_id}' timed out"
            + (f" (last status: {last_status})" if last_status else "")
        )
        self.video_id = video_id
        self.last_status = last_status
```

- [ ] **Step 2: Commit**

```bash
git add sdks/python/litegen/errors.py
git commit -m "feat(sdks/python): add typed error classes"
```

---

### Task 18: Python polling helpers (sync + async)

**Files:**
- Create: `sdks/python/litegen/polling.py`

- [ ] **Step 1: Write `polling.py`**

```python
"""Polling helpers for async video generation."""
from __future__ import annotations

import asyncio
import time
from typing import Awaitable, Callable, Protocol

from .errors import LiteGenPollingTimeoutError


class _VideoResponseProtocol(Protocol):
    id: str
    status: str
    progress: int
    video_url: str | None


_TERMINAL = {"completed", "failed", "cancelled"}


def wait_for_video_completion(
    video_id: str,
    get_status: Callable[[str], _VideoResponseProtocol],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> _VideoResponseProtocol:
    """Synchronously poll until the video reaches a terminal status.

    `get_status` is called with the video id and should return a video
    response object with a `.status` field that is one of the
    `GenerationStatus` values.
    """
    deadline = time.monotonic() + timeout
    last: _VideoResponseProtocol | None = None
    while True:
        if time.monotonic() > deadline:
            raise LiteGenPollingTimeoutError(
                video_id, last.status if last is not None else None
            )
        last = get_status(video_id)
        status = _status_value(last.status)
        if status in _TERMINAL:
            return last
        time.sleep(interval)


async def async_wait_for_video_completion(
    video_id: str,
    get_status: Callable[[str], Awaitable[_VideoResponseProtocol]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> _VideoResponseProtocol:
    """Async version of `wait_for_video_completion`."""
    deadline = time.monotonic() + timeout
    last: _VideoResponseProtocol | None = None
    while True:
        if time.monotonic() > deadline:
            raise LiteGenPollingTimeoutError(
                video_id, last.status if last is not None else None
            )
        last = await get_status(video_id)
        status = _status_value(last.status)
        if status in _TERMINAL:
            return last
        await asyncio.sleep(interval)


def _status_value(status: object) -> str:
    """Tolerate both plain-string and Enum-typed status fields."""
    return getattr(status, "value", str(status))
```

- [ ] **Step 2: Commit**

```bash
git add sdks/python/litegen/polling.py
git commit -m "feat(sdks/python): add sync + async video polling helpers"
```

---

### Task 19: Python sync client

**Files:**
- Create: `sdks/python/litegen/client.py`

- [ ] **Step 1: Write `client.py`**

```python
"""Synchronous LiteGen SDK client."""
from __future__ import annotations

import os
from typing import Any, Mapping

import httpx

from . import _generated as _gen  # type: ignore[attr-defined]
from .errors import (
    LiteGenAPIError,
    LiteGenTimeoutError,
    LiteGenValidationError,
)
from .polling import wait_for_video_completion


# Re-export commonly used pydantic models from the generated package.
# (Concrete re-exports live in `litegen/__init__.py`; this module deliberately
# does not import them here to avoid circular import issues.)


class LiteGenClient:
    """Synchronous client for the LiteGen HTTP API.

    Example:
        >>> client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
        >>> img = client.images.generate(prompt="a cat", model="openai/dall-e-3")
    """

    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        timeout: float = 60.0,
        default_headers: Mapping[str, str] | None = None,
    ) -> None:
        self._api_key = api_key or os.environ.get("LITEGEN_API_KEY")
        self._base_url = (base_url or os.environ.get("LITEGEN_BASE_URL") or "http://localhost:4000").rstrip("/")
        self._timeout = timeout
        headers: dict[str, str] = {"Content-Type": "application/json"}
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"
        if default_headers:
            headers.update(default_headers)
        self._http = httpx.Client(base_url=self._base_url, headers=headers, timeout=timeout)

        self.images = _ImagesNamespace(self)
        self.videos = _VideosNamespace(self)
        self.models = _ModelsNamespace(self)
        self.health = _HealthNamespace(self)
        self.stats = _StatsNamespace(self)
        self.logs = _LogsNamespace(self)
        self.keys = _KeysNamespace(self)
        self.cache = _CacheNamespace(self)

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "LiteGenClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    # ── transport ──────────────────────────────────────────────────────────
    def _request(self, method: str, path: str, *, json: Any = None, params: Any = None) -> Any:
        try:
            resp = self._http.request(method, path, json=json, params=params)
        except httpx.TimeoutException as e:
            raise LiteGenTimeoutError(str(e)) from e
        if resp.status_code >= 400:
            self._raise_for_status(resp)
        if resp.headers.get("content-type", "").startswith("application/json"):
            return resp.json()
        return resp.content

    @staticmethod
    def _raise_for_status(resp: httpx.Response) -> None:
        try:
            payload = resp.json()
            detail = payload.get("error", {}) if isinstance(payload, dict) else {}
        except Exception:
            detail = {}
        message = detail.get("message") or resp.text or f"HTTP {resp.status_code}"
        err_type = detail.get("type") or "api_error"
        code = detail.get("code")
        provider_error = detail.get("provider_error")
        cls = LiteGenValidationError if err_type == "validation_error" else LiteGenAPIError
        raise cls(
            resp.status_code,
            message,
            type=err_type,
            code=str(code) if code is not None else None,
            provider_error=provider_error,
        )


# ── namespaces ─────────────────────────────────────────────────────────────


class _ImagesNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def generate(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/images/generations", json=_clean(req))

    def estimate_cost(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/images/cost", json=_clean(req))


class _VideosNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def generate(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/videos/generations", json=_clean(req))

    def estimate_cost(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/videos/cost", json=_clean(req))

    def get_status(self, video_id: str) -> Any:
        return self._c._request("GET", f"/v1/videos/{video_id}")

    def wait_for_completion(
        self,
        video_id: str,
        *,
        interval: float = 2.0,
        timeout: float = 300.0,
    ) -> Any:
        class _Holder:
            def __init__(self, payload: dict[str, Any]) -> None:
                self.id = payload.get("id", video_id)
                self.status = payload.get("status", "pending")
                self.progress = payload.get("progress", 0)
                self.video_url = payload.get("video_url")
                self._raw = payload

        def _fetch(vid: str) -> Any:
            return _Holder(self.get_status(vid))

        result = wait_for_video_completion(video_id, _fetch, interval=interval, timeout=timeout)
        return result._raw  # type: ignore[attr-defined]


class _ModelsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self) -> list[Any]:
        resp = self._c._request("GET", "/v1/models")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    def get(self, model_id: str) -> Any:
        return self._c._request("GET", f"/v1/models/{model_id}")


class _HealthNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def check(self) -> Any:
        return self._c._request("GET", "/health")

    def live(self) -> Any:
        return self._c._request("GET", "/health/live")


class _StatsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def get(self) -> Any:
        return self._c._request("GET", "/v1/stats")


class _LogsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, page: int = 1, per_page: int = 50) -> Any:
        return self._c._request("GET", "/v1/logs", params={"page": page, "per_page": per_page})


class _KeysNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def create(self, name: str) -> Any:
        return self._c._request("POST", "/v1/keys", json={"name": name})

    def list(self) -> list[Any]:
        resp = self._c._request("GET", "/v1/keys")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    def revoke(self, key_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/keys/{key_id}")


class _CacheNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def clear(self) -> Any:
        return self._c._request("DELETE", "/v1/cache")


def _clean(req: dict[str, Any]) -> dict[str, Any]:
    """Drop None values so server defaults are honored."""
    return {k: v for k, v in req.items() if v is not None}
```

Note on typing: the methods return `Any` here because the generated pydantic models are not directly usable as response models without parsing. A future iteration may convert responses into pydantic models via `Model.model_validate(payload)`. For now, raw dicts give callers a stable, simple shape. The public `__init__.py` re-exports the pydantic models so users can opt into typed parsing themselves.

- [ ] **Step 2: Commit**

```bash
git add sdks/python/litegen/client.py
git commit -m "feat(sdks/python): implement sync LiteGenClient with namespaced methods"
```

---

### Task 20: Python async client

**Files:**
- Create: `sdks/python/litegen/async_client.py`

- [ ] **Step 1: Write `async_client.py`**

```python
"""Asynchronous LiteGen SDK client."""
from __future__ import annotations

import os
from typing import Any, Mapping

import httpx

from .errors import (
    LiteGenAPIError,
    LiteGenTimeoutError,
    LiteGenValidationError,
)
from .polling import async_wait_for_video_completion


class AsyncLiteGenClient:
    """Asynchronous client for the LiteGen HTTP API.

    Example:
        >>> async with AsyncLiteGenClient(api_key="lg-...") as client:
        ...     img = await client.images.generate(prompt="...", model="...")
    """

    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        timeout: float = 60.0,
        default_headers: Mapping[str, str] | None = None,
    ) -> None:
        self._api_key = api_key or os.environ.get("LITEGEN_API_KEY")
        self._base_url = (base_url or os.environ.get("LITEGEN_BASE_URL") or "http://localhost:4000").rstrip("/")
        self._timeout = timeout
        headers: dict[str, str] = {"Content-Type": "application/json"}
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"
        if default_headers:
            headers.update(default_headers)
        self._http = httpx.AsyncClient(base_url=self._base_url, headers=headers, timeout=timeout)

        self.images = _AsyncImages(self)
        self.videos = _AsyncVideos(self)
        self.models = _AsyncModels(self)
        self.health = _AsyncHealth(self)
        self.stats = _AsyncStats(self)
        self.logs = _AsyncLogs(self)
        self.keys = _AsyncKeys(self)
        self.cache = _AsyncCache(self)

    async def close(self) -> None:
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncLiteGenClient":
        return self

    async def __aexit__(self, *_exc: object) -> None:
        await self.close()

    async def _request(self, method: str, path: str, *, json: Any = None, params: Any = None) -> Any:
        try:
            resp = await self._http.request(method, path, json=json, params=params)
        except httpx.TimeoutException as e:
            raise LiteGenTimeoutError(str(e)) from e
        if resp.status_code >= 400:
            self._raise_for_status(resp)
        if resp.headers.get("content-type", "").startswith("application/json"):
            return resp.json()
        return resp.content

    @staticmethod
    def _raise_for_status(resp: httpx.Response) -> None:
        try:
            payload = resp.json()
            detail = payload.get("error", {}) if isinstance(payload, dict) else {}
        except Exception:
            detail = {}
        message = detail.get("message") or resp.text or f"HTTP {resp.status_code}"
        err_type = detail.get("type") or "api_error"
        code = detail.get("code")
        provider_error = detail.get("provider_error")
        cls = LiteGenValidationError if err_type == "validation_error" else LiteGenAPIError
        raise cls(
            resp.status_code,
            message,
            type=err_type,
            code=str(code) if code is not None else None,
            provider_error=provider_error,
        )


def _clean(req: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in req.items() if v is not None}


class _AsyncImages:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def generate(self, **req: Any) -> Any: return await self._c._request("POST", "/v1/images/generations", json=_clean(req))
    async def estimate_cost(self, **req: Any) -> Any: return await self._c._request("POST", "/v1/images/cost", json=_clean(req))


class _AsyncVideos:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def generate(self, **req: Any) -> Any: return await self._c._request("POST", "/v1/videos/generations", json=_clean(req))
    async def estimate_cost(self, **req: Any) -> Any: return await self._c._request("POST", "/v1/videos/cost", json=_clean(req))
    async def get_status(self, video_id: str) -> Any: return await self._c._request("GET", f"/v1/videos/{video_id}")
    async def wait_for_completion(
        self, video_id: str, *, interval: float = 2.0, timeout: float = 300.0
    ) -> Any:
        class _Holder:
            def __init__(self, payload: dict[str, Any]) -> None:
                self.id = payload.get("id", video_id)
                self.status = payload.get("status", "pending")
                self.progress = payload.get("progress", 0)
                self.video_url = payload.get("video_url")
                self._raw = payload

        async def _fetch(vid: str) -> Any:
            return _Holder(await self.get_status(vid))

        result = await async_wait_for_video_completion(video_id, _fetch, interval=interval, timeout=timeout)
        return result._raw  # type: ignore[attr-defined]


class _AsyncModels:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def list(self) -> Any:
        resp = await self._c._request("GET", "/v1/models")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp
    async def get(self, model_id: str) -> Any: return await self._c._request("GET", f"/v1/models/{model_id}")


class _AsyncHealth:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def check(self) -> Any: return await self._c._request("GET", "/health")
    async def live(self) -> Any: return await self._c._request("GET", "/health/live")


class _AsyncStats:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def get(self) -> Any: return await self._c._request("GET", "/v1/stats")


class _AsyncLogs:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def list(self, *, page: int = 1, per_page: int = 50) -> Any:
        return await self._c._request("GET", "/v1/logs", params={"page": page, "per_page": per_page})


class _AsyncKeys:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def create(self, name: str) -> Any: return await self._c._request("POST", "/v1/keys", json={"name": name})
    async def list(self) -> Any:
        resp = await self._c._request("GET", "/v1/keys")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp
    async def revoke(self, key_id: str) -> Any: return await self._c._request("DELETE", f"/v1/keys/{key_id}")


class _AsyncCache:
    def __init__(self, c: AsyncLiteGenClient) -> None: self._c = c
    async def clear(self) -> Any: return await self._c._request("DELETE", "/v1/cache")
```

- [ ] **Step 2: Commit**

```bash
git add sdks/python/litegen/async_client.py
git commit -m "feat(sdks/python): implement AsyncLiteGenClient"
```

---

### Task 21: Python public exports

**Files:**
- Modify: `sdks/python/litegen/__init__.py`

- [ ] **Step 1: Replace placeholder**

```python
"""LiteGen Python SDK.

>>> from litegen import LiteGenClient
>>> client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
>>> img = client.images.generate(prompt="a cat", model="openai/dall-e-3")
"""
from __future__ import annotations

__version__ = "0.1.0"

from .client import LiteGenClient
from .async_client import AsyncLiteGenClient
from .errors import (
    LiteGenError,
    LiteGenAPIError,
    LiteGenValidationError,
    LiteGenTimeoutError,
    LiteGenPollingTimeoutError,
)
from .polling import (
    wait_for_video_completion,
    async_wait_for_video_completion,
)

# Re-export pydantic models from the generated package for users who want
# typed request/response shapes. Names are pulled lazily so a missing model
# (due to spec drift) doesn't break the whole import.
def _reexport_generated() -> list[str]:
    import importlib
    exported: list[str] = []
    try:
        mod = importlib.import_module("litegen._generated.models")
    except ModuleNotFoundError:
        return exported

    candidates = [
        "ImageGenerationRequest", "ImageGenerationResponse", "ImageResult",
        "VideoGenerationRequest", "VideoGenerationResponse",
        "ReferenceImage", "RefImageKind",
        "GenerationStatus", "MediaType", "CostSource", "RoutingStrategy",
        "ModelInfo", "ModelSchema", "ModelCapabilities", "ModelPricing",
        "CostEstimate", "UsageInfo",
        "ProxyStats", "RequestLog", "ProviderHealth",
        "ApiKey", "ApiKeyInfo", "ApiKeyListResponse", "ApiKeyCreatedResponse",
        "ErrorResponse", "ErrorDetail",
        "HealthResponse", "LivenessResponse", "CacheStatus",
        "ModelListResponse", "CacheClearedResponse", "RevokeKeyResponse",
    ]
    g = globals()
    for name in candidates:
        if hasattr(mod, name):
            g[name] = getattr(mod, name)
            exported.append(name)
    return exported


__all__ = [
    "__version__",
    "LiteGenClient",
    "AsyncLiteGenClient",
    "LiteGenError",
    "LiteGenAPIError",
    "LiteGenValidationError",
    "LiteGenTimeoutError",
    "LiteGenPollingTimeoutError",
    "wait_for_video_completion",
    "async_wait_for_video_completion",
] + _reexport_generated()
```

- [ ] **Step 2: Smoke test**

```bash
cd sdks/python
python3 -c "import litegen; print(litegen.LiteGenClient, litegen.AsyncLiteGenClient)"
```

Expected: Prints class objects with no errors.

- [ ] **Step 3: Commit**

```bash
git add sdks/python/litegen/__init__.py
git commit -m "feat(sdks/python): wire up public exports"
```

---

### Task 22: Python unit tests

**Files:**
- Create: `sdks/python/tests/__init__.py`
- Create: `sdks/python/tests/test_client.py`

- [ ] **Step 1: Install dev deps**

```bash
cd sdks/python && python3 -m pip install --user -e ".[dev]"
```

- [ ] **Step 2: Create `tests/__init__.py`**

Empty file:
```python
```

- [ ] **Step 3: Write `tests/test_client.py`**

```python
"""Unit tests for the LiteGen Python SDK."""
from __future__ import annotations

import pytest
import respx
from httpx import Response

from litegen import (
    AsyncLiteGenClient,
    LiteGenAPIError,
    LiteGenClient,
    LiteGenValidationError,
)


@respx.mock
def test_images_generate_sends_auth_and_body() -> None:
    route = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(200, json={
            "created": 1, "data": [{"url": "https://x.png", "content_type": "image/png", "index": 0}],
            "model": "m", "provider": "p", "id": "img-1",
        })
    )
    client = LiteGenClient(api_key="lg-test")
    resp = client.images.generate(prompt="a cat", model="m")
    assert route.called
    sent = route.calls[0].request
    assert sent.headers["authorization"] == "Bearer lg-test"
    assert b"a cat" in sent.content
    assert resp["id"] == "img-1"


@respx.mock
def test_api_error_decoded() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(400, json={"error": {"message": "bad", "type": "validation_error", "code": "400"}})
    )
    client = LiteGenClient(api_key="lg")
    with pytest.raises(LiteGenValidationError) as exc:
        client.images.generate(prompt="", model="m")
    assert exc.value.status == 400
    assert exc.value.type == "validation_error"


@respx.mock
def test_provider_error_raised_as_api_error() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(502, json={"error": {"message": "upstream broke", "type": "provider_error"}})
    )
    client = LiteGenClient()
    with pytest.raises(LiteGenAPIError) as exc:
        client.images.generate(prompt="x", model="m")
    assert exc.value.status == 502
    assert not isinstance(exc.value, LiteGenValidationError)


@respx.mock
def test_video_wait_for_completion_polls_until_done() -> None:
    statuses = ["processing", "processing", "completed"]
    def _resp(_req):
        s = statuses.pop(0)
        return Response(200, json={
            "id": "vid-1", "status": s, "model": "m", "provider": "p",
            "progress": 100 if s == "completed" else 50, "created": 1,
            "video_url": "https://done.mp4" if s == "completed" else None,
        })
    respx.get("http://localhost:4000/v1/videos/vid-1").mock(side_effect=_resp)
    client = LiteGenClient()
    result = client.videos.wait_for_completion("vid-1", interval=0.01, timeout=5.0)
    assert result["status"] == "completed"
    assert result["video_url"] == "https://done.mp4"


@respx.mock
def test_models_list_unwraps_envelope() -> None:
    respx.get("http://localhost:4000/v1/models").mock(
        return_value=Response(200, json={"object": "list", "data": [{"id": "m1"}]})
    )
    client = LiteGenClient()
    models = client.models.list()
    assert models == [{"id": "m1"}]


@pytest.mark.asyncio
@respx.mock
async def test_async_client_generate() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(200, json={
            "created": 1, "data": [], "model": "m", "provider": "p", "id": "img-1",
        })
    )
    async with AsyncLiteGenClient(api_key="lg") as client:
        resp = await client.images.generate(prompt="x", model="m")
    assert resp["id"] == "img-1"
```

- [ ] **Step 4: Run tests**

```bash
cd sdks/python && python3 -m pytest -v
```

Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add sdks/python/tests/
git commit -m "test(sdks/python): unit tests for sync + async clients and polling"
```

---

### Task 23: Python examples

**Files:**
- Create: `sdks/python/examples/generate_image.py`
- Create: `sdks/python/examples/generate_video.py`

- [ ] **Step 1: `examples/generate_image.py`**

```python
"""Example: generate an image with the LiteGen Python SDK."""
import os

from litegen import LiteGenClient


def main() -> None:
    client = LiteGenClient(
        base_url=os.environ.get("LITEGEN_BASE_URL", "http://localhost:4000"),
        api_key=os.environ.get("LITEGEN_API_KEY"),
    )
    result = client.images.generate(
        prompt="a serene mountain landscape at sunset, oil painting",
        model="openai/dall-e-3",
        size="1024x1024",
        quality="hd",
        n=1,
    )
    print("Image URL:", result["data"][0].get("url"))
    print("Provider:", result["provider"])
    if result.get("usage"):
        print("Cost USD:", result["usage"]["cost_usd"])


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: `examples/generate_video.py`**

```python
"""Example: generate a video and wait for completion."""
import os

from litegen import LiteGenClient


def main() -> None:
    client = LiteGenClient(
        base_url=os.environ.get("LITEGEN_BASE_URL", "http://localhost:4000"),
        api_key=os.environ.get("LITEGEN_API_KEY"),
    )
    job = client.videos.generate(
        prompt="a timelapse of clouds drifting over a quiet city",
        model="runway/gen-3",
        duration_seconds=5,
    )
    print("Job started:", job["id"])
    final = client.videos.wait_for_completion(job["id"], interval=5.0, timeout=600.0)
    if final["status"] == "completed":
        print("Video URL:", final.get("video_url"))
    else:
        print("Generation failed:", final.get("error"))


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Commit**

```bash
git add sdks/python/examples/
git commit -m "docs(sdks/python): add runnable image and video examples"
```

---

## Phase 5 — CI integration

### Task 24: CI workflow

**Files:**
- Create or modify: `.github/workflows/sdks.yml`

- [ ] **Step 1: Check whether `.github/workflows/` exists**

```bash
ls /Users/joeviscardi/source/repos/litegen/.github/workflows/ 2>/dev/null || echo "no workflows yet"
```

- [ ] **Step 2: Write the workflow**

Create `.github/workflows/sdks.yml`:

```yaml
name: SDKs

on:
  pull_request:
    paths:
      - "sdks/**"
      - "litegen-core/**"
      - ".github/workflows/sdks.yml"
  push:
    branches: [main]

jobs:
  typescript:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: "npm"
          cache-dependency-path: sdks/typescript/package-lock.json
      - name: Install
        working-directory: sdks/typescript
        run: npm ci
      - name: Typecheck
        working-directory: sdks/typescript
        run: npm run typecheck
      - name: Test
        working-directory: sdks/typescript
        run: npm test
      - name: Build
        working-directory: sdks/typescript
        run: npm run build

  python:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: "3.11"
      - name: Install
        working-directory: sdks/python
        run: |
          python -m pip install --upgrade pip
          pip install -e ".[dev]"
      - name: Test
        working-directory: sdks/python
        run: python -m pytest -v
      - name: Mypy
        working-directory: sdks/python
        run: python -m mypy litegen || true   # non-blocking for first iteration

  codegen-up-to-date:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
      - uses: actions/setup-python@v5
        with:
          python-version: "3.11"
      - uses: dtolnay/rust-toolchain@stable
      - name: Install openapi-python-client
        run: pip install openapi-python-client
      - name: Regenerate SDKs
        run: ./sdks/scripts/regen-all.sh
        env:
          LITEGEN_MODELS_DIR: ${{ github.workspace }}/models
      - name: Check for drift
        run: |
          if ! git diff --exit-code sdks/; then
            echo "SDK codegen is out of date. Run ./sdks/scripts/regen-all.sh and commit."
            exit 1
          fi
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/sdks.yml
git commit -m "ci: add SDK build, test, and codegen-drift workflows"
```

---

## Verification checklist (run after all tasks complete)

- [ ] `cargo build -p litegen-core` succeeds.
- [ ] `cargo test -p litegen-core` succeeds.
- [ ] `cd sdks/typescript && npm run typecheck && npm test && npm run build` all succeed.
- [ ] `cd sdks/python && python -m pytest -v` succeeds.
- [ ] `./sdks/scripts/regen-all.sh` runs without errors and produces no `git diff` afterwards.
- [ ] `curl -s http://localhost:4000/openapi.json | python3 -m json.tool | head -20` shows valid OpenAPI 3.x JSON with all expected paths.
- [ ] `node -e "const { LiteGenClient } = require('./sdks/typescript/dist/index.cjs'); console.log(new LiteGenClient())"` runs cleanly (smoke test of built artifact).

## Notes for the executor

- **DRY across SDKs:** the sync and async Python clients have near-identical namespace structures by design — keeping them flat and parallel is intentional; do not try to share a base class across them (it makes the async/sync method signatures harder to type).
- **Generated code is a black box:** never hand-edit anything under `litegen/_generated/` or `src/generated/`. If shape changes are needed, change the Rust types and regenerate.
- **TS responses are `Any`-shaped at the boundary in Python:** the spec explicitly accepts this for v1. A future task may swap raw dicts for pydantic-parsed responses. Don't preemptively add validation here.
- **Commit cadence:** commit after each task. Do not batch commits.
- **No CI workflow execution required during local implementation** — the workflow runs on push.
