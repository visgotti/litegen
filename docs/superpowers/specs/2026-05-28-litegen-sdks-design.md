# LiteGen SDKs — TypeScript & Python

**Date:** 2026-05-28
**Status:** Approved, ready for implementation planning
**Scope:** First-party SDKs for the LiteGen HTTP API, with full type/enum parity to `litegen-core`.

## Goal

Ship two first-party SDKs — TypeScript and Python — that:

- Cover the entire `litegen-core` HTTP surface (image gen, video gen, cost estimation, models, health, stats, logs, API key management, cache).
- Stay in lock-step with `litegen-core` types and enums through OpenAPI-driven codegen, not hand-mirrored types.
- Present an ergonomic, OpenAI-SDK-style facade over the generated low-level client.
- Live in-repo under `sdks/` and are buildable, testable, and lintable in CI; they are **not published** to npm/PyPI in this initial iteration.

## Why now

The dashboard's existing `dashboard/src/api.ts` is a thin hand-rolled fetch wrapper with loose typing (`media_type: string`, `capabilities: Record<string, unknown>`). External consumers (and the dashboard itself, over time) need a first-class typed client. Building it now — while the `litegen-core` HTTP surface is still settling — is cheaper than retrofitting later, and forces us to keep the API types `utoipa`-annotated and machine-described.

## Non-goals

- No retry/backoff logic in the SDK. The server already handles provider-side fallback chains and retries; adding SDK-side retries would compound costs and re-route latency-sensitive failures the server intentionally surfaced.
- No streaming. There are no streaming endpoints.
- No framework integrations (React Query hooks, Django adapters, etc).
- No telemetry, analytics, or usage reporting from the SDKs.
- No CLI tools.
- No publishing pipeline yet. The release workflow stays manual/disabled until we want to cut a public version.

## Prerequisite work in `litegen-core`

Two small additions land first because the SDKs cannot be built without them:

### 1. OpenAPI aggregator + `GET /openapi.json`

All handlers in [litegen-core/src/api/handlers.rs](litegen-core/src/api/handlers.rs) already carry `#[utoipa::path(...)]` annotations, but no `#[derive(utoipa::OpenApi)]` aggregator currently exists. Add one that lists:

- Every handler path (`generate_image`, `estimate_image_cost`, `generate_video`, `estimate_video_cost`, `get_video_status` (new — see below), `list_models`, `get_model_schema`, `health_check`, `liveness`, `get_stats`, `get_logs`, `create_api_key`, `list_api_keys`, `revoke_api_key`, `clear_cache`).
- Every component schema from [litegen-core/src/types/mod.rs](litegen-core/src/types/mod.rs) that already derives `ToSchema`.

Expose at `GET /openapi.json` via the existing axum router (no auth required — same as `/health/live`). Optionally serve swagger-ui at `/docs` for human inspection.

### 1a. `ToSchema` coverage for types referenced by handlers but not currently annotated

Two gaps exist today that will cause codegen to produce `unknown` / `Any` for real response payloads:

- [`ModelSchema`](litegen-core/src/capabilities/schema.rs) and its component types (`ModelCapabilityFlags`, `PromptSpec`, `ParamSpec`, `SizeSpec`, `RefInputSpec`, `RefRoleSpec`, `RefProviderFormat`, `ModelPricing`) — returned by `GET /v1/models/{id}` but currently derive only `Serialize, Deserialize`. Add `ToSchema` to each, and update the `#[utoipa::path]` on `get_model_schema` to declare the response body.
- `list_api_keys` and `create_api_key` currently return ad-hoc `serde_json::json!({...})` shapes. Introduce typed response structs (e.g. `ApiKeyListItem`, `ApiKeyCreatedResponse`) that derive `Serialize` + `ToSchema`, return those instead of inline JSON, and reference them in the `#[utoipa::path]` `responses` clauses.
- Similarly, the `clear_cache` and `health_check` inline JSON responses should become small typed structs so the SDKs get real types instead of `Record<string, unknown>` / `dict[str, Any]`.

This is mechanical work but explicitly part of the prerequisite scope, not an afterthought.

### 2. `GET /v1/videos/{id}` status endpoint

`POST /v1/videos/generations` returns an ID for an async job, but there is currently no way to poll its status. Add a handler that:

- Takes a video generation ID as a path parameter.
- Returns the current `VideoGenerationResponse` (same response type as initial generation — `id`, `status`, `model`, `provider`, `video_url?`, `progress`, `error?`, `usage?`, `created`).
- Returns 404 if no such generation exists.
- Reads from whatever in-memory or persistent state the router uses to track in-flight video jobs. (The router already needs to track these to report status to providers; this just exposes it.)

This endpoint is what `waitForCompletion` / `wait_for_video_completion` polls.

## Repository layout

```
sdks/
  README.md                       # how to install, use, regenerate
  openapi.json                    # committed snapshot of /openapi.json
  scripts/
    fetch-openapi.sh              # boots litegen-core (or uses LITEGEN_BASE_URL), saves openapi.json
    regen-all.sh                  # fetch-openapi.sh + run codegen for both SDKs
  typescript/
    package.json                  # name: @litegen/sdk
    tsconfig.json
    tsup.config.ts
    src/
      generated/
        schema.d.ts               # openapi-typescript output — committed
      client.ts                   # LiteGenClient — ergonomic wrapper
      errors.ts                   # LiteGenAPIError, LiteGenTimeoutError
      polling.ts                  # waitForVideoCompletion
      index.ts                    # public exports
    test/
      client.test.ts              # mocked-fetch unit tests
      integration.test.ts         # gated by LITEGEN_INTEGRATION=1
    examples/
      generate-image.ts
      generate-video.ts
  python/
    pyproject.toml                # name: litegen (fallback: litegen-sdk)
    codegen.yml                   # openapi-python-client config
    litegen/
      __init__.py                 # public re-exports
      client.py                   # LiteGenClient (sync)
      async_client.py             # AsyncLiteGenClient
      errors.py                   # LiteGenAPIError, LiteGenValidationError, LiteGenTimeoutError
      polling.py                  # wait_for_video_completion, async_wait_for_video_completion
      _generated/                 # openapi-python-client output — committed
    tests/
      test_client.py
      test_integration.py         # gated by LITEGEN_INTEGRATION env var
    examples/
      generate_image.py
      generate_video.py
```

## Codegen toolchain

### TypeScript: `openapi-typescript`

- Generates a single `schema.d.ts` file containing TypeScript types for every component schema and every operation. **Zero runtime** — types are stripped at build.
- Generated types live at `sdks/typescript/src/generated/schema.d.ts`. Hand-written client imports from it: `import type { components, operations } from "./generated/schema"`.
- Type helpers re-exported in `index.ts` so consumers write `import type { ImageGenerationRequest, GenerationStatus } from "@litegen/sdk"` instead of digging through the generated namespace.

### Python: `openapi-python-client`

- Generates a full client package (pydantic v2 models + httpx-based sync + async call functions, one per operation) into `sdks/python/litegen/_generated/`.
- Hand-written `LiteGenClient` / `AsyncLiteGenClient` wraps the generated package to present a cleaner, namespaced surface (`client.images.generate(...)` instead of `from _generated.api.images import generate_image; generate_image.sync(client=...)`).
- All generated pydantic models are re-exported at the top of `litegen/__init__.py` so callers use `from litegen import ImageGenerationRequest, GenerationStatus`.

### Why these two

- Both are well-maintained, popular, fast.
- No Java/JVM dependency (`openapi-generator` would require it).
- Smallest runtime footprint in TS (types-only).
- Modern, idiomatic output in Python (pydantic v2 + httpx).

## Codegen flow

- `sdks/scripts/fetch-openapi.sh`:
  - If `LITEGEN_BASE_URL` is set, curls `${LITEGEN_BASE_URL}/openapi.json`.
  - Otherwise starts `cargo run --bin litegen` in the background, waits for `/health/live`, curls `/openapi.json`, kills the process.
  - Writes pretty-printed JSON to `sdks/openapi.json`.
- `sdks/scripts/regen-all.sh`:
  - Calls `fetch-openapi.sh`.
  - Runs `npx openapi-typescript sdks/openapi.json -o sdks/typescript/src/generated/schema.d.ts`.
  - Runs `openapi-python-client generate --path sdks/openapi.json --config sdks/python/codegen.yml --overwrite` writing into `sdks/python/litegen/_generated/`.

### CI guardrails

A `sdk-codegen-up-to-date` job:

1. Runs `sdks/scripts/regen-all.sh`.
2. Fails if `git diff --exit-code sdks/` reports changes.

This catches the case where someone edits a `ToSchema`-derived type in `litegen-core` and forgets to regenerate.

A separate `sdk-build` job:

- TypeScript: `npm ci && npm run build && npm test && tsc --noEmit`.
- Python: `pip install -e .[dev] && pytest && mypy --strict litegen`.

Integration tests (`LITEGEN_INTEGRATION=1`) run nightly, not per-PR.

## TypeScript SDK design

### Public API

```ts
import { LiteGenClient, GenerationStatus } from "@litegen/sdk";

const client = new LiteGenClient({
  baseUrl: "http://localhost:4000",
  apiKey: process.env.LITEGEN_API_KEY!,
  // optional
  fetch: customFetch,        // for Cloudflare Workers, Deno, etc.
  timeoutMs: 60_000,
  defaultHeaders: { "X-Trace-Id": "..." },
});

const img = await client.images.generate({
  prompt: "a serene mountain landscape at sunset",
  model: "openai/dall-e-3",
  size: "1024x1024",
  quality: "hd",
});

const video = await client.videos.generate({
  prompt: "...",
  model: "runway/gen-3",
  duration_seconds: 5,
});
const final = await client.videos.waitForCompletion(video.id, {
  intervalMs: 2_000,
  timeoutMs: 5 * 60_000,
});
if (final.status === GenerationStatus.Completed) {
  console.log(final.video_url);
}
```

### Method surface

- `client.images.generate(req)` → `ImageGenerationResponse`
- `client.images.estimateCost(req)` → `CostEstimate`
- `client.videos.generate(req)` → `VideoGenerationResponse`
- `client.videos.estimateCost(req)` → `CostEstimate`
- `client.videos.getStatus(id)` → `VideoGenerationResponse`
- `client.videos.waitForCompletion(id, opts?)` → `VideoGenerationResponse` (final state)
- `client.models.list()` → `ModelInfo[]`
- `client.models.get(id)` → `ModelSchema`
- `client.health.check()` → health response
- `client.health.live()` → `{ status: "ok" }`
- `client.stats.get()` → `ProxyStats`
- `client.logs.list({ page?, perPage? })` → `PaginatedResponse<RequestLog>`
- `client.keys.create(name)` → `{ key, prefix, name, created_at }`
- `client.keys.list()` → `ApiKeyInfo[]`
- `client.keys.revoke(id)` → `{ revoked: boolean }`
- `client.cache.clear()` → `{ cleared: boolean }`

### Enums

`GenerationStatus`, `MediaType`, `CostSource`, `RoutingStrategy`, `RefImageKind` are re-exported as `const`-object unions:

```ts
export const GenerationStatus = {
  Pending: "pending",
  Processing: "processing",
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
} as const;
export type GenerationStatus = typeof GenerationStatus[keyof typeof GenerationStatus];
```

This avoids the TypeScript `enum` quirks (numeric back-mapping, structural compatibility issues with string literals from the API).

### Errors

```ts
class LiteGenAPIError extends Error {
  status: number;
  type: string;
  code?: string;
  providerError?: unknown;
}
class LiteGenTimeoutError extends Error {}  // for waitForCompletion
```

`LiteGenAPIError` is thrown on any non-2xx. Never returns half-typed `unknown`.

### Build

- `tsup` produces ESM + CJS + `.d.ts` outputs from a single `src/index.ts` entry.
- `package.json` declares both via `exports` map.
- Target: ES2022. Node 18+. Works in modern browsers, Deno, Bun, Cloudflare Workers (because of injectable fetch).

### Why not bundle `openapi-fetch`

`openapi-fetch` is a popular runtime companion to `openapi-typescript`. It's small (~6kb) and would save us writing a low-level transport. We deliberately skip it because:

- It exposes type-level operation paths (`client.GET("/v1/images/generations")`) rather than the named, namespaced methods (`client.images.generate(...)`) we want.
- Wrapping it gives the same ergonomic surface as just calling fetch directly, but adds a dependency.

If the hand-written wrapper grows complex, revisit.

## Python SDK design

### Public API

```python
from litegen import LiteGenClient, AsyncLiteGenClient, GenerationStatus

client = LiteGenClient(
    api_key="lg-...",
    base_url="http://localhost:4000",
    # optional
    timeout=60.0,
    default_headers={"X-Trace-Id": "..."},
)

img = client.images.generate(
    prompt="a serene mountain landscape at sunset",
    model="openai/dall-e-3",
    size="1024x1024",
    quality="hd",
)

video = client.videos.generate(
    prompt="...",
    model="runway/gen-3",
    duration_seconds=5,
)
final = client.videos.wait_for_completion(video.id, interval=2.0, timeout=300.0)
if final.status == GenerationStatus.COMPLETED:
    print(final.video_url)
```

`AsyncLiteGenClient` exposes the same surface with `await`:

```python
async with AsyncLiteGenClient(api_key="...") as client:
    img = await client.images.generate(prompt="...", model="...")
```

### Method surface

Same as the TS surface, named in snake_case:

- `client.images.generate(...)`, `client.images.estimate_cost(...)`
- `client.videos.generate(...)`, `client.videos.estimate_cost(...)`, `client.videos.get_status(id)`, `client.videos.wait_for_completion(id, interval=2.0, timeout=300.0)`
- `client.models.list()`, `client.models.get(id)`
- `client.health.check()`, `client.health.live()`
- `client.stats.get()`, `client.logs.list(page=1, per_page=50)`
- `client.keys.create(name)`, `client.keys.list()`, `client.keys.revoke(id)`
- `client.cache.clear()`

### Models and enums

All pydantic v2 models from `_generated/models/` are re-exported at the package top level so callers don't see the `_generated` prefix:

```python
from litegen import (
    ImageGenerationRequest,
    ImageGenerationResponse,
    VideoGenerationRequest,
    VideoGenerationResponse,
    ReferenceImage,
    RefImageKind,
    GenerationStatus,
    MediaType,
    CostSource,
    RoutingStrategy,
    ModelInfo,
    ModelSchema,
    CostEstimate,
    UsageInfo,
    ProxyStats,
    RequestLog,
    ApiKey,
    ErrorResponse,
)
```

Enums are real `enum.StrEnum` (Python 3.11+) or `str`-subclass enums for 3.10 compatibility, generated by `openapi-python-client` from the snake_case JSON values.

### Errors

```python
class LiteGenAPIError(Exception):
    status: int
    type: str
    code: str | None
    provider_error: Any | None

class LiteGenValidationError(LiteGenAPIError): ...  # 400 with validation_error type
class LiteGenTimeoutError(Exception): ...           # for wait_for_completion
```

### Build

- PEP 621 `pyproject.toml`, `hatchling` backend.
- Python 3.10+ (pydantic v2 requires it; matches OpenAI SDK floor).
- Optional `[dev]` extras: `pytest`, `pytest-asyncio`, `mypy`, `httpx[testing]`, `respx`.

## Testing strategy

### Unit tests (run per PR)

- TypeScript: mock `fetch` via `vitest`'s `vi.fn()` or `msw`. Assert request URL, method, headers (auth, content-type, custom headers), body shape. Decode mocked responses through the typed client and assert returned shape.
- Python: `httpx.MockTransport` (or `respx`) for sync; same for async. Same coverage.
- Cover: happy path for each method, auth header injection, default header merging, error decoding (`LiteGenAPIError` on 400/401/429/500/502), timeout/abort behavior, polling logic (mock 3 polls returning `processing`, then one returning `completed`).

### Integration tests (run nightly, `LITEGEN_INTEGRATION=1`)

- Spin up litegen-core with a test config (mock provider configured in `models/` test fixtures that returns deterministic responses).
- Exercise: image generation, model listing, key creation/listing/revocation.
- Not run per-PR — provider mock fixtures may be flaky, and we don't want SDK tests blocking core PRs.

### Type checking

- TypeScript: `tsc --noEmit` against `src/` and `test/`.
- Python: `mypy --strict litegen` and `mypy litegen examples/`.

### Examples

- `sdks/typescript/examples/generate-image.ts`, `generate-video.ts` — runnable scripts demonstrating the happy path.
- `sdks/python/examples/generate_image.py`, `generate_video.py` — same.
- Examples are type-checked but not executed in CI.

## Package metadata

- **npm:** `@litegen/sdk`. Leaves room for siblings (`@litegen/cli`, `@litegen/dashboard-types`).
- **PyPI:** `litegen` (preferred). If the name is taken at first publish, fall back to `litegen-sdk` and revisit later. Both names are reserved in the spec; the actual choice is deferred until first publish.

## Open risks

- **`/openapi.json` completeness.** If any handler is missing a `#[utoipa::path]` annotation or any type is missing `ToSchema`, codegen will silently omit it. Mitigation: the integration tests hit every endpoint, which will surface gaps fast.
- **Generated code drift.** Without the CI guard, generated code and Rust types can desync. Mitigation: the `sdk-codegen-up-to-date` CI job.
- **`litegen-core` startup for `fetch-openapi.sh`.** The script relies on litegen-core building cleanly with no `models/` config. Mitigation: ship a minimal test config under `sdks/scripts/litegen.codegen.yaml` if the default config requires provider keys.
- **Video status endpoint state.** The new `GET /v1/videos/{id}` needs somewhere to read job state from. If the router currently fires-and-forgets video generation, we may need to add lightweight in-memory tracking. Confirm during implementation; this design assumes the router can already report status.

## Out-of-scope follow-ups (for later, not this spec)

- Publishing to npm/PyPI with semver + changelog.
- React Query hooks (`@litegen/react`).
- Go and Ruby SDKs.
- Webhook receiver helpers.
- A CLI built on top of the TS SDK.
