🌐 **[litegen.ai](https://litegen.ai)**  ·  📖 **Docs, SDKs & API reference → [visgotti.github.io/litegen](https://visgotti.github.io/litegen/)**

# LiteGen

> The universal proxy for AI image, video, and 3D-model generation. Like LiteLLM, but for multimedia.

LiteGen provides a unified API gateway for the major image and video generation providers, plus a third media family — `model3d` (text/image → mesh, rigging and animation). Route requests across OpenAI GPT Image, Google Gemini (Nano Banana), Black Forest Labs FLUX, Stability AI, Runway, Luma, Kling, MiniMax, ByteDance Seedance, fal, Replicate and more ([22 providers](#supported-providers)) — with automatic fallback, weighted load balancing, caching, cost tracking, and a real-time dashboard.

## Features

- **Unified API** — OpenAI-compatible REST endpoints for image, video & 3D-model generation (`/v1/images/*`, `/v1/videos/*`, [`/v1/models3d/*`](#generate-3d-model))
- **20 Providers** — every catalogued model of each one is listed in [Supported Providers](#supported-providers); an operator adds more at runtime (Gateway → Models)
- **Smart Routing** — Fallback chains, weighted round-robin, lowest-cost, lowest-latency
- **Caching** — In-memory cache with configurable TTL to avoid duplicate generations; with Redis set, a shared tier behind it lets instances share hits
- **Cost Tracking** — Per-request cost estimation and aggregate spend analytics
- **API Key Management** — Create/revoke keys, weighted key pools for rate limit distribution
- **MCP Server** — Claude Code, ChatGPT, Codex, Cursor, VS Code and other MCP clients use the AI API at `/mcp` and the management API at `/admin/mcp`, with the same permissions as the HTTP API ([docs/mcp](docs/mcp/README.md))
- **Stored outputs** — Outputs kept in litegen's store, in an app's own S3 bucket, or nowhere at all, and handed out as signed, proxied or public links per app ([Stored outputs](#stored-outputs-who-may-open-them))
- **Hooks** — Python, Node or Bun hooks that check a call before auth or rewrite its answer ([Hooks](#hooks-custom-auth-request-and-response-rewrites))
- **Observability** — Request logging, Prometheus metrics, structured tracing
- **React Dashboard** — Real-time monitoring, provider health, request logs, cost charts
- **Easy Config** — YAML config + env vars, auto-discovers provider keys
- **Self-Hosted** — Single binary + Docker, SQLite or Postgres

## What's new in v0.2

- **Capability registry with per-model schemas** — YAML-driven model declarations define allowed parameters, sizes, aspect ratios, and reference-image roles; unknown params are rejected (strict mode) or silently dropped (lax mode).
- **`reference_images` tagged-union** — Send reference images as URLs, base64 blobs, or multipart file uploads using `{type: "url"|"base64"|"blob", value: "..."}`.
- **Strict validation with per-request lax opt-out** — Default `strict: true` rejects unsupported parameters and fields the request has no such name for; set `strict: false` to drop them and get an `X-Litegen-Dropped-Params` response header.
- **DB-backed multi-key auth: scopes, quotas, RPM rate limits** — Create API keys with per-key USD budget caps, requests-per-minute throttles, and CSV scope sets (`generate`, `read`, `admin`).
- **DB-backed video generations + polling endpoint** — Video jobs are persisted to the `generations` table; poll status via `GET /v1/generations/{id}`. A background poller updates status every 5 s.
- **Per-key webhook delivery on video completion** — When a generation reaches a terminal status (`completed`, `failed`, `cancelled`), the owning key's `webhook_url` receives a signed `POST` with the generation payload (`X-Litegen-Signature: sha256=<hex>`). Retries 3 times with exponential back-off.
- **Request metadata, spend per tag** — Send `metadata` (a string map, at most 1 KB) on any generation or segmentation: it is kept on the row, echoed on every answer and webhook, and filters `GET /v1/logs` and `GET /v1/stats` as `metadata.<key>=<value>`, so spend splits by project or asset.
- **Cancel at the vendor, refund on cancel or failure** — `PATCH /v1/generations/{id}` asks the vendor to stop (fal, Replicate and Runway have a cancel) and reports `vendor_cancelled`; a job that is cancelled or fails gives its charge back to the key's quota and every limit it counted against.
- **OpenTelemetry OTLP gRPC export** — Set `OTEL_EXPORTER_OTLP_ENDPOINT` to ship traces to any OTel collector.
- **Prometheus `/metrics`** — Expose Prometheus-format metrics at `GET /metrics`; `observability.metrics_path` moves it, `observability.metrics_enabled: false` switches it off, and `observability.metrics_require_auth: true` puts it behind a key (see [docs/mcp/self-hosting.md](docs/mcp/self-hosting.md#observability)).
- **Dashboard CRUD UI for keys + model schema panel** — Create, update, and revoke API keys from the React dashboard; inspect full model capability schemas in the Models panel.

## Running locally

The minimum required env vars:

```bash
# Port the proxy listens on (default: 4000)
export LITEGEN__SERVER__PORT=4000

# Master API key — if set, all requests need Authorization: Bearer <key>
# Omit to run in dev mode (no auth)
export LITEGEN__MASTER_KEY=your-secret-master-key

# Database URL. Left out, litegen keeps SQLite in data/litegen.db, created on
# the first start; Postgres for prod:
# export LITEGEN_DATABASE_URL=postgres://user:pass@localhost:5432/litegen

# Directory containing provider YAML model definitions (default: ./models)
export LITEGEN_MODELS_DIR=./models

# Provider API keys (as needed)
export OPENAI_API_KEY=sk-...
export RUNWAY_API_KEY=...
```

Then start the backend and dashboard:

```bash
# Backend: run it from the repository root, where models/ and litegen.yaml are
cargo build --release --manifest-path litegen-core/Cargo.toml
./litegen-core/target/release/litegen

# Dashboard (separate terminal)
cd dashboard
npm install
npm run dev
```

## Quick Start

### Docker (recommended)

```bash
git clone https://github.com/litegen/litegen.git
cd litegen
# litegen on port 4000, with Postgres beside it
LITEGEN_MASTER_KEY=your-secret-key docker compose up -d
```

`http://localhost:4000/health/live` answers `{"status":"ok"}` once it is up. Compose runs the API only:
the dashboard is a separate app ([From Source](#from-source)).

Provider keys reach the container through `environment:` in `docker-compose.yml`
(`OPENAI_API_KEY: "${OPENAI_API_KEY:-}"`), or through a config file: copy `litegen.example.yaml`
to `litegen.yaml`, fill it in, and uncomment the `./litegen.yaml:/app/litegen.yaml:ro` line under
`volumes:`. A variable under `environment:` beats the same setting in the file.

### Run the Docker image

As with LiteLLM's image, settings come from environment variables, from a config file, or both.
A variable beats the same setting in the file.

```bash
docker build -t litegen .

# Environment variables only: the database and the kept files go in the litegen-data volume
docker run -d --name litegen -p 4000:4000 -v litegen-data:/app/data \
  -e LITEGEN__MASTER_KEY=sk-change-me -e OPENAI_API_KEY=sk-... litegen

# A config file, read as /app/litegen.yaml
docker run -d --name litegen -p 4000:4000 -v litegen-data:/app/data \
  -v "$PWD/litegen.yaml:/app/litegen.yaml:ro" litegen

# A config file anywhere, named by --config (or LITEGEN_CONFIG_FILE_PATH)
docker run -d --name litegen -p 4000:4000 -v litegen-data:/app/data \
  -v "$PWD/conf:/config:ro" litegen --config /config/litegen.yaml
```

- `/app/data` holds the SQLite database (`/app/data/litegen.db`, the default when no database is
  set) and the kept files (`/app/data/assets`). Give it a named volume, as above: a new container
  on an anonymous volume starts empty.
- The image runs as uid 1000, so a host directory mounted at `/app/data` must be writable by uid 1000.
- Apart from `LITEGEN_MODELS_DIR`, the image sets no litegen setting, so a config file's
  `server.port` or `database_url` is used as written. It listens on `0.0.0.0:4000` unless a setting
  says otherwise.
- Without a master key it refuses to start, since it binds `0.0.0.0`
  ([Security](#security-authentication-on-public-binds)).
- On a remote host or behind a proxy, set `LITEGEN__SERVER__PUBLIC_BASE_URL` to the origin clients
  reach it on.
- For Postgres, set `LITEGEN_DATABASE_URL`, or the `LITEGEN_DATABASE_HOST`, `_USER`, `_PASSWORD` and
  `_NAME` parts ([docs/scaling.md](docs/scaling.md)).
- The image carries no Python, Node or Bun: for hooks, build on it with the runtime they use
  ([docs/hooks.md](docs/hooks.md#deploying)).
- `docker stop` stops it gracefully: it handles SIGTERM.

CI builds the image and runs it these ways with `.github/scripts/docker-smoke.sh`: the three runs
above, the volume through a restart and a new container, uid 1000, the refusal without a master
key, Postgres from the parts, and `docker stop`. Where litegen looks for its config file, and which
setting wins, are in [docs/configuration.md](docs/configuration.md).

### From Source

```bash
# Backend: run it from the repository root, where models/ and litegen.yaml are
# (or from elsewhere with --config <file> and LITEGEN_MODELS_DIR)
cargo build --release --manifest-path litegen-core/Cargo.toml
./litegen-core/target/release/litegen

# Dashboard (separate terminal)
cd dashboard
npm install
npm run dev
```

### Environment Variables

Set provider keys via env vars (no config file needed):

```bash
export OPENAI_API_KEY=sk-...
export STABILITY_API_KEY=sk-...
export REPLICATE_API_TOKEN=r8_...
export GOOGLE_API_KEY=...
export FAL_KEY=...
export RUNWAY_API_KEY=...
export LUMA_API_KEY=...
```

Every provider's variable is in the [environment variable reference](#environment-variable-reference).
A vendor's variable beats the key in `litegen.yaml`, and `LITEGEN__PROVIDERS__<P>__API_KEY`
beats both; where litegen finds its config file, and the whole order, are in
[docs/configuration.md](docs/configuration.md).

## API Reference

### Generate Image
```bash
curl -X POST http://localhost:4000/v1/images/generations \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "a serene mountain landscape at sunset",
    "model": "openai/gpt-image-2",
    "size": "1024x1024",
    "quality": "high"
  }'
```

`resolution` picks a model's output tier on one scale for every vendor (`0.5K`, `0.75K`, `1K`,
`1.5K`, `2K`, `4K`; each model lists its own in `params.resolution`, default `1K`), where `size`
is exact pixels: Gemini image's `imageSize`, Kling image's and FLUX 3's tiers are all `resolution`.

`output_format` (`png`, `webp` or `jpeg`) and `background` (`transparent` or `opaque`) are request
fields where a model's listing offers them (`output_formats`, `supports_transparent_background`;
`lossy_only` marks a model with no PNG). Every image's `content_type` is read from its bytes.

A model whose `task` is `multiview` (`tripo/multiview`) turns one `init` image into one image per
entry of `views` (`front`, `left`, `back`, `right`, …), each returned with its `view` and `yaw_deg`:
the camera's position around +Y from the subject's front (+Z) towards +X, so front 0, left 90,
back 180 and right 270. The call is billed once, however many views come back.

A model whose listing says `supports_expand` (`stability/outpaint`) grows the init image's canvas
with `expand: { "left": 64, "top": 0, "right": 64, "bottom": 128 }` (pixels a side); every image
reports the `width` and `height` it came back at.

A model whose listing says `native_pixel_art` (`pixellab/pixflux`) draws at native size; `palette:
["#rrggbb", …]` is every pixel's colour where `supports_forced_palette` and a hint elsewhere
(Recraft), and `directions: 8` returns one image per facing, each labelled `direction`.

On any model taking a mask or a region, `preserve_unmasked: true` pastes the init image's pixels
back outside the edit area so they come back byte-identical (`feather_px` softens the edge inside
it). A model whose `task` is `background_swap` (`recraft/replace-background`) redraws the init
image's background from the prompt.

`layout` places objects and words on a text-to-image canvas by boxes in canvas fractions
(`supports_layout`, `layout_kinds`: Recraft V3 lays out single words; FLUX 3 Image and Klein take
objects and lines of text as BFL's bounding boxes), and `mask_prompt` names the
area an edit replaces in text instead of a drawn mask (`supports_mask_prompt`:
`stability/search-and-replace`).

A model whose listing says `supports_background_removal` (`fal/bria-rmbg-2`, `fal/birefnet-v2`,
`recraft/remove-background`, `stability/remove-background`) is an edit that takes only an `init`
image and returns it as an RGBA PNG with its background transparent.

### Strict requests

Every generation, quote (`/cost`) and segmentation request is checked against its model before
anything is billed. With `strict: true`, the default, a param the model does not take and a field
the request has no such name for are refused with 400 `param_unsupported`, naming the field (one
inside an object by its path, such as `reference_images[0].weight`) and, where it fits, the field
it is likely meant to be (`duration` for `duration_seconds`). With `strict: false` they are dropped,
and the `X-Litegen-Dropped-Params` response header names them. A bad value for a param the model
does take is refused either way. A field the model passes through (its `extra_allowlist` in
`GET /v1/models/{id}`, such as `user` on the OpenAI image models) may be sent at the top level, as
OpenAI's SDK sends it, and is forwarded as if sent in `extra`. `n` is 1: litegen makes one image,
video or mesh per request, and refuses more.

### Segment an Image
```bash
curl -X POST http://localhost:4000/v1/images/segmentations \
  -H "Content-Type: application/json" \
  -d '{
    "model": "fal/sam-3",
    "image": "<base64 PNG, JPEG, WebP or GIF>",
    "prompts": [{ "type": "box", "x": 120, "y": 40, "w": 200, "h": 160 },
                { "type": "point", "x": 210, "y": 90, "label": 0 }]
  }'
```

Answered at once with `masks: [{ png_base64, score, box }]`: each a grayscale PNG the image's size,
white where selected, with the box litegen measured around it, best score first. Point and box
prompts together select one object (`label: 0` leaves a spot out); a text prompt
(`{ "type": "text", "text": "visor" }`) selects every instance of what it names and is sent alone.
Models whose `task` is `segment` (`fal/sam-3`, `fal/sam-3.1`) list `supports_segmentation` and the
prompt types they take as `segmentation_prompts`; they are billed once per request.

### Generate Video
```bash
curl -X POST http://localhost:4000/v1/videos/generations \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "a timelapse of clouds over a city",
    "model": "runway/gen4.5",
    "duration_seconds": 5
  }'
```

Video operations are models too: an `extend`, `retake` or `video_edit` model works on a
`source_video` (a generation id, a URL or base64) — `direction` and `duration_seconds` to extend,
`time_range` and `replace` to retake a window — and the finished job reports `segments`, which
stretches came from the source and which were generated.

A model whose listing says `supports_keyframes` (`pixverse/v5-multi-transition`) takes `keyframe`
reference images, each with `time_s` — seconds from the start of the clip — and passes through each
at its time; `first_frame` and `last_frame` still name 0 s and the end. `keyframe_timing` says
whether times are free (`any`) or evenly spaced (`even`), and bad times are refused before billing.

### Generate 3D Model

3D generation is **always async**: the `POST` returns a job (`status: "pending"`, `progress: 0`) and the
client polls `GET /v1/models3d/{id}` until a terminal status. A completed generation carries **exactly
one `mesh` asset** per requested format — plus optional `preview`/`texture` assets, and `package` assets
when asked for — and every asset `url` is absolute.

```bash
# Submit → {"id": "litegen-3d-<uuid>", "status": "pending", "progress": 0, ...}
curl -X POST http://localhost:4000/v1/models3d/generations \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "a low-poly fox",
    "model": "mock/mesh-3d",
    "target_polycount": 20000
  }'

# Poll until status is completed / failed / cancelled
curl http://localhost:4000/v1/models3d/litegen-3d-<uuid>

# Estimate cost without generating
curl -X POST http://localhost:4000/v1/models3d/cost \
  -H "Content-Type: application/json" \
  -d '{"prompt": "a low-poly fox", "model": "mock/mesh-3d"}'
```

Every `/cost` quote, the quota reservation and the charge use the same price: the model's
`pricing.base_cost_usd` for its default request, adjusted by its `pricing.variable_pricing` rules
for the request at hand (per second of video, per megapixel, per clip, multipliers and add-ons).

The mode comes from the reference images' roles: none is text-to-3D, `init` is image-to-3D, and
`view-<name>` roles (`view-front`, `view-left`, `view-back`, `view-right`, …) are multiview-to-3D.
A multiview request always needs `view-front`; a model whose `ref_inputs.view_order` is set reads
its views by position, so each view needs every view before it in that order. Rodin takes up to five
views of any names, in any order.

Operations on a mesh are models too, named by their `task` and given the mesh as `source_model` (a
generation id, a URL or base64): `rig`, `animate`, `texture` (from a `prompt` and/or a `style`
reference image, with `pbr`, `texture_resolution` and `keep_uvs`), `remesh` (`target_polycount`,
`topology`) and `segment`. A segmented mesh's manifest lists its `parts`, one glTF node each, named
with the vendor's label or `part_<n>`.

Any 3D request may also ask litegen to place the delivered mesh (`supports_placement`): `ground: true`
puts its lowest point at y = 0, `center: true` centres its bounds on x and z, and `height_m` scales it
to that height — one root-node transform, so skins and clips are unchanged.

It may ask for packages too. With `output_packages: true`, every requested container that is a file
plus sidecars (an `obj` with its `.mtl` and textures, a `dae`, `usda` or `usdc` with its textures, a `gltf`
with its `.bin` and images) also arrives as one zip of them: an asset of kind `package`, whose `format`
is the container inside, served as `application/zip`. The `mesh` assets stay as they are. A request
that keeps nothing (`X-Litegen-Keep-Outputs: false`) can't ask for packages (400 `option_needs_keeping`).

A vendor's KTX2 (Basis Universal) textures arrive as PNG in every container, as WebP ones do. With
`ktx2_textures: "keep"`, the `glb` and `gltf` containers keep them byte for byte through
`KHR_texture_basisu`, which those files then require, and every other container still gets PNG. A
request that keeps nothing can't ask for `keep` (400 `option_needs_keeping`). The converter takes the
same choice as `?ktx2_textures=keep`.

`usdc` is a container too: USD's binary crate, written as crate version 0.8.0 and served as
`application/octet-stream`, because IANA registers no media type for it. Two options shape the USD files
litegen writes. `usdz_root: "usda" | "usdc"` chooses the root layer of a `usdz`: `usda`, the default, is
text, and `usdc` is the binary crate, as Apple's own packagers write it. `usd_clips: "all" | "first"`
chooses the animation clips of a `usda`, `usdc` or `usdz`: `all` keeps every clip as an alternative (one
SkelAnimation per clip, picked by a variant set named `Animation`; every clip's skeletal and blend-shape
animation is kept, rigid node motion is the first clip's only and the other clips' is reported, and a scene
whose clips are all rigid writes as `first`), and `first` keeps the first and reports the rest. Unset, `usd_clips` is `all` for `usda` and `usdc` and `first` for `usdz`, until AR Quick
Look's behaviour with variants is checked on a device. A request that keeps nothing can't ask for
`usdz_root: "usdc"` or `usd_clips: "all"` (400 `option_needs_keeping`). The converter takes the same
choices as `?usdz_root=` and `?usd_clips=`.

> The built-in mocks need no key: `mock/mesh-3d` and `mock/all-params-3d` emit a real glTF 2.0
> binary cube, while `mock/fail-3d` emits no mesh at all — it terminates `failed`, by design, to
> exercise the error path. Vendor 3D models are listed under [Supported Providers](#supported-providers).

### Convert a 3D Model

`POST /v1/utils/models3d/convert?from=<format>&to=<format>` converts a mesh you already have, with no
model and no charge: the body is the raw file, the answer is the converted one, and
`X-Litegen-Conversion-Losses` names what the target couldn't hold. `GET /v1/utils/models3d/formats`
lists the containers. The answer is one file by default; `package=zip` answers with a zip of the file and
its sidecars instead (`model.<to>.zip`), which keeps an OBJ's materials and textures and the textures of
Collada, USDA and USDC.

```bash
curl --data-binary @fox.glb "http://localhost:4000/v1/utils/models3d/convert?from=glb&to=obj&package=zip" -o fox.obj.zip
```

### List Models
```bash
curl http://localhost:4000/v1/models
```

Each model says whether its provider can serve you (`is_available`), where it is in its vendor's
lifecycle (`status`: `active`, `deprecated` or `retired`, with `deprecated_at`, `retires_at` and
`replaced_by`) and its limits (`max_reference_images`, `max_duration_seconds`). A model is no longer
served from the day after its `retires_at`. `catalog_version` changes whenever any listed field does.

### Health Check
```bash
curl http://localhost:4000/health
```

### Get Stats
```bash
curl http://localhost:4000/v1/stats
```

### Request Logs
```bash
curl "http://localhost:4000/v1/logs?page=1&per_page=50"
```

### Management (budgets, end users, spend, runtime config)
```bash
# A $50/month budget for an app, alerting at $40
curl -X PUT http://localhost:4000/v1/apps/$APP_ID/limits -H "Authorization: Bearer $KEY" \
  -d '{"max_budget_usd": 50, "soft_budget_usd": 40, "budget_duration": "1mo"}'
# Spend this month by end user (requests tagged with x-litegen-end-user)
curl "http://localhost:4000/v1/spend/report?group_by=end_user&from=2026-10-01" -H "Authorization: Bearer $KEY"
```
Also covers key/org/member budgets, model allowlists, rpm and parallel caps, end-user
blocking, and runtime models, provider keys, routes and markup. See
[docs/management-api.md](docs/management-api.md).

### Late and async images
An image job the vendor is still running when litegen stops waiting is
delivered later, never lost: the 504 names the generation, and
`GET /v1/generations/{id}` serves the image when it's done. Send
`Prefer: respond-async` (or `wait=N`) to get a `202` at once instead. See
[docs/async-images.md](docs/async-images.md).

### Provider config (keys, proxies, headers, model names)
Each vendor takes the same fields, in `litegen.yaml`, env vars or
`PUT /v1/admin/providers/{name}`: `api_key(s)`, `api_base` (route a vendor
through a proxy or AI gateway), `extra_headers` (sent with every call to
the vendor's API, never with downloads) and `model_mapping` (send a
catalog model under another name: a new version, a deployment, a
fine-tune). See [docs/provider-config.md](docs/provider-config.md).

### Gateway settings (cache, retries, timeouts)
Set the cache, retries, the vendor timeout and the circuit breaker in
`litegen.yaml`, then change them live from the dashboard (Gateway → Settings)
or `PUT /v1/admin/settings`:
- every replica picks up a change within 15 s;
- a model route can turn caching on or off for its models;
- a request can opt out with `Cache-Control: no-cache` / `no-store`;
- every image answer says `X-Litegen-Cache: hit | miss | bypass`;
- retries back off with jitter and honour a vendor's `Retry-After`.

See [docs/gateway-settings.md](docs/gateway-settings.md).

### Hooks (custom auth, request and response rewrites)
Run your own Python, Node or Bun (TypeScript included) code around every
generation call, named in `litegen.yaml`:
```yaml
hooks:
  - path: hooks/custom_auth.py     # pre_call: check your token, inject a litegen key
  - path: hooks/rewrite.ts         # pre_call: alias a model
  - path: hooks/tag_results.mjs    # post_call: tag the finished answer
```
`pre_call` sees the raw request before litegen authenticates it; `post_call`
sees the finished response, including the completed result of a video or 3D
job. See [docs/hooks.md](docs/hooks.md) and the runnable
[examples/hooks/](examples/hooks/).

### Stored outputs: who may open them
litegen keeps every image, video and 3D file it produces, and hands out its
URLs in the app's **access mode**:
- `public`, the default: the stored URL;
- `signed`: links that expire;
- `private`: litegen's file route, `GET /v1/generations/{id}/assets/{index}`.
  It wants a key, and checks which keys may open the file: the one that made
  it, any of its app's, or any of its org's.

```yaml
storage_access:
  mode: signed                  # public | signed | private
  owner_scope: key              # key | app | org
  signed_url_ttl_seconds: 3600  # 60 to 604800
```
Override the default gateway-wide on the dashboard (Gateway → Settings) or
per app (Organization → Access to stored files, `PUT /v1/apps/{app_id}/access`).
A `file_access` hook can narrow it further, for example to each of your
customers' own files. See [docs/storage-access.md](docs/storage-access.md).

## Model Routing

Configure sophisticated routing in `litegen.yaml`:

```yaml
model_routes:
  # Fallback: try OpenAI first, then Stability
  - model: "gpt-image-2"
    strategy: fallback
    deployments:
      - provider: openai
        max_retries: 2
        timeout_seconds: 120
      - provider: stability

  # Weighted: 75% Fal, 25% Replicate
  - model: "fal/*"
    strategy: weighted_round_robin
    deployments:
      - provider: fal
        weight: 3
      - provider: replicate
        weight: 1

  # Cheapest provider first
  - model: "*"
    strategy: lowest_cost
    deployments:
      - provider: openai
      - provider: stability
      - provider: replicate
```

Each deployment runs on the calling org's own key for that deployment's
provider when the org stored one, and otherwise on the gateway's key for it,
as a direct request to that provider would. An org's key for one vendor never
goes to another vendor's deployment.

## Architecture

```
┌──────────────┐     ┌──────────────────────────────────────────┐
│   Clients    │────▶│             LiteGen Proxy                │
│  (REST API)  │     │                                          │
└──────────────┘     │  ┌────────┐  ┌────────┐  ┌──────────┐   │
                     │  │ Router │─▶│ Cache  │─▶│ Provider │   │
                     │  │        │  │ (moka) │  │ Registry │   │
                     │  └────────┘  └────────┘  └──────────┘   │
                     │                               │          │
                     │  ┌──────┐  ┌──────────┐       ▼          │
                     │  │  DB  │  │ Metrics  │  ┌──────────┐   │
                     │  │(SQLx)│  │(Prom/OTel│  │ Image    │   │
                     │  └──────┘  └──────────┘  │ Video    │   │
                     │                          │ 3D       │   │
                     │  ┌────────────────────┐  │ adapters │   │
                     │  │  React Dashboard   │  │ (20      │   │
                     │  │  (Vite + Recharts) │  │ vendors) │   │
                     │  └────────────────────┘  │          │   │
                     │                          └──────────┘   │
                     └──────────────────────────────────────────┘
```

## Project Structure

```
litegen/
├── litegen-core/              # Rust backend (the gateway)
│   ├── src/
│   │   ├── main.rs            # Server entrypoint
│   │   ├── lib.rs             # Library exports
│   │   ├── api/               # REST handlers, auth middleware, OpenAPI, CORS
│   │   ├── access/            # Who may open a stored output: signed, proxied and public links
│   │   ├── auth/              # Permissions, roles, OAuth sign-in
│   │   ├── capabilities/      # The model catalog: schemas, validation, pricing rules
│   │   ├── config/            # YAML + env config loading
│   │   ├── db/                # SQLite/Postgres persistence
│   │   ├── hooks/             # Pre- and post-call hooks (Python, Node, Bun)
│   │   ├── management/        # Budgets, limits, end users, runtime settings
│   │   ├── mcp/               # MCP server (/mcp, /admin/mcp) and its OAuth
│   │   ├── mesh/              # Standalone 3D format conversion (glb, obj, fbx, usdz, …)
│   │   ├── providers/         # Vendor adapters
│   │   │   ├── auth/          # Shared auth schemes (bearer, SigV4, TC3, JWT)
│   │   │   ├── image/         # OpenAI, Google, Stability, BFL, Ideogram, Recraft, PixelLab, Leonardo,
│   │   │   │                  # Runway, Luma, Kling, MiniMax, ByteDance, Hunyuan, TokenHub, Tripo, fal, Replicate, Bedrock
│   │   │   ├── video/         # Runway, Luma, Kling, MiniMax, ByteDance, Hunyuan, TokenHub, Vidu, PixVerse,
│   │   │   │                  # Leonardo, fal, Replicate, Google (Veo), OpenAI, Bedrock
│   │   │   └── model3d/       # fal, Tripo, Meshy
│   │   ├── proxy/             # Router, registry, cache, poller, output delivery
│   │   └── types/             # Shared types + OpenAPI schemas
│   ├── migrations/            # SQL migrations (sqlite/, postgres/)
│   └── tests/                 # Integration tests
├── models/                    # The model catalog, one YAML file per provider
├── dashboard/                 # React + Vite dashboard (Playground, Providers, Gateway, Keys, …)
├── sdks/                      # TypeScript and Python SDKs, generated from sdks/openapi.json
├── apps/
│   ├── landing/               # litegen.ai (Next.js), its data derived from models/
│   └── price-api/             # Pricing service (NestJS), seeded from models/
├── docs/                      # Feature docs (storage, hooks, gateway settings, MCP, …)
├── scripts/model-drift/       # Weekly check of vendors' models and params against the catalog
├── examples/hooks/            # Example hooks
├── deploy/                    # Production compose files and Caddy config
├── Dockerfile                 # Multi-stage build
├── docker-compose.yml         # One-command deployment
└── litegen.example.yaml       # Example configuration
```

## Supported Providers

Several vendors serve more than one family under one provider name — e.g. a single
`fal` provider routes `fal/flux-dev` (image), `fal/ltx-2.3` (video) and `fal/trellis` (3D).
The model ID prefix (`<provider>/<model>`) selects the provider, and the model's row its family.

| Provider | Image | Video | 3D | Models |
|----------|-------|-------|----|--------|
| OpenAI | ✅ | — | — | GPT Image 2.5 (Sunburst, Flare), GPT Image 2 · _(Sora 2 and the Videos API shut down 2026-09-24)_ |
| Google | ✅ | — | — | Gemini 3 Pro, 3.1 Flash and 3.1 Flash Lite image (Nano Banana 2 and 2 Lite), Nano Banana 2.1, Gemini 2.5 Flash image · _Veo: see the note below_ |
| Black Forest Labs | ✅ | — | — | FLUX 3 Image and Image Klein, FLUX.2 pro, max, flex and klein 9B, FLUX 1.1 pro and pro Ultra, FLUX.1 dev, FLUX.1 Kontext pro and max |
| Stability AI | ✅ | — | 🔜 | SD 3.5 Large and Large Turbo, SD 3.5 Medium, SDXL, Stable Image Core and Ultra, Inpaint, Outpaint, Search and Replace, Remove Background · _(Stable Video hosted API discontinued)_ |
| Ideogram | ✅ | ✅ | — | Ideogram 3.0 and 4.0 (each with Turbo and Quality), 4.5 and 4.5 Precise Edit, P-Image; resold: Nano Banana 2 and Pro · resold: Kling 3.0 Standard, MiniMax H3, Seedance 2.0 and 2.5 (text, first/last frame, reference images) |
| Recraft | ✅ | — | — | Recraft V2, V3, V4.1, V4.1 Pro and V4.1 Flash, raster and vector · Remove and Replace Background |
| PixelLab | ✅ | — | — | Pixflux native pixel art (forced palette, never upscaled) |
| Leonardo.Ai | ✅ | ✅ | — | Diffusion XL · Motion 2.0, Veo 3.1 and 3.1 Fast, Kling 2.5 Turbo |
| Runway | ✅ | ✅ | — | Gen-4 Image and Image Turbo; resold: GPT Image 2, 2.5 and 2.6 (Flare, Sunburst), Gemini 2.5 Flash, 3 Pro, 3.1 Flash and 3.1 Flash Lite image, Nano Banana 2.1, Seedream 5.0 Pro, Lite and Flash, Grok Imagine Image 2, Bria FIBO 1.5 and FIBO Edit 1.5, Ideogram 4.0, 4.5 and Vector, Muse Image · Gen-4 Turbo, Gen-4.5; resold: Veo 3.1 and 3.1 Fast, Gemini Omni Flash 1.0, 1.1 and 1.2, Kling 2.5 Turbo Pro, 3.0 and O3 (Standard, Pro, 4K), 4.0 and 4.0 Flash, MiniMax H3 and H3 Max, Seedance 2.0, 2.0 Fast, 2.0 Mini and 2.5, Grok Imagine Video 1.5 and 1.5 Lite, Wan 3.0 and 3.0 Prime, HappyHorse 1.0 |
| Luma | ✅ | ✅ | — | Photon, Photon Flash · Dream Machine, Ray 2, Ray Flash 2 |
| Kling AI | ✅ | ✅ | — | Kling Image v2.1 and v3 · Kling Video v2.5 Turbo, v2.6, v3, v3 Turbo and v4 |
| MiniMax | ✅ | ✅ | — | image-01 · Hailuo 02, 2.3 and 2.3 Fast, H3, H3 Max, H3 Highspeed and H3 Lite, T2V-01 Director, S2V-01 |
| ByteDance (BytePlus ModelArk) | ✅ | ✅ | — | Seedream 4.0, 4.5, 5.0 Lite, 5.0 Pro and 5.0 Flash · Seedance 1.0 Pro and Pro Fast, 2.0 Mini |
| Tencent Hunyuan (Tencent Cloud API) | ✅ | ✅ | — | Hunyuan Image · Hunyuan Video and Video I2V — for buyers of the old platform, which Tencent closed to new purchases as it moves Hunyuan to TokenHub |
| Amazon Bedrock | ✅ | ✅ | — | On your AWS account (us-west-2): Stability AI's Stable Diffusion 3.5 Large, Stable Image Core, Stable Image Ultra · Luma Ray 2 (the MP4 goes to your S3 bucket, `s3_output_uri`, and litegen reads it back) |
| Tencent TokenHub | ✅ | ✅ | 🔜 | Hy-Image 3.5 Preview (to 4K, 20 images); resold: Seedream 5.0 Pro and Flash, Kling Image 3.0, O1 and 3.0 Omni, Vidu Image Q2 · Hy-Video 1.5, WAND-Vega Video 1.0 Lite, YouTu Video 2.0; resold: Kling 2.5 Turbo, 2.6, 3.0, 3.0 Turbo, 3.0 Omni, O1 and 4.0, MiniMax Hailuo 2.3 and 2.3 Fast, H3 and H3 Max, PixVerse V6, C1 and V5.6, Vidu Q4 Preview, Q3, Q3 Pro, Q3 Turbo, Q2, Q2 Pro and Q2 Turbo · 3D (Hunyuan 3D 3.1, resold Tripo, Rodin) waits for a live run |
| Vidu | — | ✅ | — | Vidu Q1 and Q1 Classic, 2.0, Q2, Q2 Turbo, Q2 Pro and Pro Fast, Q3, Q3 Mix, Q3 Pro, Pro Fast and Turbo, Q4 Preview |
| PixVerse | — | ✅ | — | PixVerse v3.5, v4.5, v5, v5 Multi-Transition, V6, V6 Max, C1 |
| Tripo | ✅ | — | ✅ | Image to multiview (front, left, back, right) · auto rig (after Tripo's free rig-check) · animation retarget |
| Meshy | — | — | ✅ | Auto rig · animation library (678 clips) |
| fal | ✅ | ✅ | ✅ | FLUX 3 Image (1K), FLUX 3 Klein and Klein base, Flux Pro, FLUX.1 Kontext pro and max, FLUX1.1 pro ultra, Redux, Fill and Erase, Flux Dev and Schnell, FLUX.2 dev (with edits), turbo, flash, pro (with outpainting), max, flex, and klein 4B and 9B with their base models, SDXL, SD 3.5 Medium, Recraft V2, V3 (with image-to-image), V4 and V4.1 (pro, vector, utility, flash), V4 style references, vectorize and upscale, AuraFlow, SAM 3 and 3.1 segmentation, Bria RMBG 2.0 and BiRefNet v2 background removal · FLUX 3 video (standard, 720p drafts and Max), LTX-2.3, LTX Video (`fal/video`) · Hyper3D Rodin, TRELLIS, Hunyuan3D v2 and 3.1 Pro, Tripo H3.1 multiview, Meshy rigging |
| Replicate | ✅ | ✅ | — | FLUX 3 Image, Flux Pro, 1.1 Pro, Dev and Schnell, SDXL, SD3, SD 3.5 Large, Large Turbo and Medium · AnimateDiff (`replicate/video`) |

**Legend:** ✅ supported in litegen · 🔜 the vendor offers a public API for this family but litegen
hasn't wired it yet (see [Roadmap](#roadmap--missing-providers)) · — no public API, or nothing to carry.
`GET /v1/models` (and the dashboard's Models page) lists every model with its params and price.

**Adapters with no catalog rows:** Google's Veo video adapter is wired, but Google has shut down
every Veo model on the Gemini API (the 3.1 previews on 2026-10-22, Veo 3.0 and 2.0 on 2026-06-30),
so no row ships and a runtime row would fail at Google. Google's named replacement,
`gemini-omni-1.1-flash`, is an Interactions API model the adapter does not speak. The dashboard's
Providers page says when a provider serves no model.

**3D (`model3d`):** generation through fal (Rodin, TRELLIS, Hunyuan3D v2 and 3.1 Pro; multiview through
Tripo H3.1, Hunyuan3D 3.1 Pro and Rodin); rigging through Tripo,
Meshy and fal's Meshy endpoint; animation through Tripo and Meshy (see
[Generate 3D Model](#generate-3d-model)). Every delivered mesh is normalised and carries a measured
manifest.

## Roadmap — Missing Providers

What a public API offers that litegen doesn't carry yet:

| Gap | Family | Why it isn't carried yet |
|-----|--------|--------------------------|
| **Meshy text- and image-to-3D** | 3D | Not wired yet: image-to-3D is one stage, text-to-3D two (a preview, then a billed refine) on the stage plumbing. Rig and animate are carried. |
| **Stability SF3D and SPAR3D** | 3D | The API is synchronous (the mesh comes back in the response), so it needs the sync shim. |
| **Tripo generation** | 3D | Each extra output format is a billed conversion at Tripo. |
| **Rodin direct** | 3D | API access is Business tier (about $1,152 a year); Rodin is carried through fal. |
| **Google Veo rows** | video | Google shut down every Veo model on the Gemini API (the last on 2026-10-22); its replacement, `gemini-omni-1.1-flash`, needs the Interactions API, which the adapter does not speak. |

**No first-party public API (reachable only indirectly / not integrable directly):**
- **Midjourney** — image + video, but UI/Discord only; no REST API.
- **Pika** — video only; no first-party API (exposed via the fal aggregator).
- **Adobe Firefly** — image + video via *Firefly Services* (enterprise/Adobe IMS auth), not a simple key.

## Continuous integration

Every push and pull request runs `.github/workflows/ci.yml`: every test suite
in the repository that needs no vendor key, on a clean Linux checkout, with
Postgres and S3, checks that generated files match their sources, and builds
the Docker image. [docs/ci.md](docs/ci.md) lists the jobs and what a red run
usually means.

## Deployment

### Quick start with docker compose

```bash
LITEGEN_MASTER_KEY=your-secret-key docker compose up -d
```

The service starts on port **4000**. The Postgres database is provisioned automatically; no manual migration step is needed.
Provider keys reach the container only through its `environment:` (or a mounted config file); see
[Docker (recommended)](#docker-recommended).

To allow browser clients through CORS:

```bash
LITEGEN_MASTER_KEY=your-secret-key LITEGEN_CORS_ORIGINS=https://your-domain docker compose up -d
```

### Security: authentication on public binds

In **single-tenant** mode (the default), a request with no credentials is granted full admin scope (the local-dev convenience bypass). To prevent an unauthenticated admin API from being silently exposed, LiteGen runs a startup self-check: if it is bound to a **non-loopback** address (e.g. the default `0.0.0.0`) in single-tenant mode with **no real `LITEGEN__MASTER_KEY`** (unset, empty, or whitespace-only), it **refuses to start** with a fatal error.

Resolve it by either:

- setting `LITEGEN__MASTER_KEY` to a strong random secret (recommended), **or**
- binding loopback-only (`LITEGEN__SERVER__HOST=127.0.0.1`) behind a reverse proxy, **or**
- explicitly opting into no-auth with `LITEGEN__ALLOW_NO_AUTH=true` (logs a loud warning; only for trusted networks / fronting auth proxies).

Hosted (multi-tenant) mode is unaffected — the no-credential bypass is disabled there, so unauthenticated requests get `401`.

### Environment variable reference

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config <file>` (argument) | No | `litegen.yaml` in the working directory | The config file to read; `-c <file>` works too. Its extension gives the format, YAML otherwise. A named file that is missing stops litegen. See [docs/configuration.md](docs/configuration.md). |
| `LITEGEN_CONFIG_FILE_PATH` (or `LITEGEN_CONFIG_FILE`) | No | — | The config file, as `--config` names it: LiteLLM's `CONFIG_FILE_PATH` with litegen's prefix. `--config` beats it. |
| `LITEGEN__MASTER_KEY` | Yes\* | — | Bearer token that grants full admin access. All API requests must include `Authorization: Bearer <value>`. \*In single-tenant mode the proxy **refuses to start** if this is unset (or blank/whitespace) while bound to a non-loopback address — see the security note below. |
| `LITEGEN__DATABASE_URL` | No | `sqlite://data/litegen.db?mode=rwc` | SQLite (`sqlite://path/to/file.db`; `?mode=rwc` creates the file and its directory) or Postgres (`postgres://user:pass@host/db`) connection URL. The default is `data/litegen.db` in the working directory (`/app/data/litegen.db` in the Docker image); a `litegen.db` left in the working directory by the old default (`sqlite://litegen.db`) is used instead, with a warning, until `data/litegen.db` exists. |
| `LITEGEN_REDIS_URL` | No | _(no Redis)_ | `redis://` or `rediss://` URL of a Redis that several instances share. Host, port, password and TLS can be set as parts instead, and the database has `LITEGEN_DATABASE_URL` and `LITEGEN_DATABASE_*` parts. See [docs/scaling.md](docs/scaling.md). |
| `LITEGEN__SERVER__HOST` | No | `0.0.0.0` | Bind address. Defaults to all interfaces; set to `127.0.0.1` to bind loopback-only behind a reverse proxy. |
| `LITEGEN__SERVER__PORT` | No | `4000` | TCP port the HTTP server listens on. |
| `LITEGEN__ALLOW_NO_AUTH` | No | `false` | Escape hatch for the startup security check. Set to `true` to run single-tenant with **no authentication** on a non-loopback bind (e.g. behind a trusted auth proxy); the server logs a loud warning instead of refusing to start. Leave unset in normal deployments. |
| `LITEGEN_MODELS_DIR` | No | `./models` | Directory containing provider YAML model capability definitions. |
| `LITEGEN_CORS_ORIGINS` | No | _(deny all)_ | Comma-separated list of allowed CORS origins, e.g. `https://app.example.com`. Leave empty to deny browser cross-origin requests. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | No | — | gRPC endpoint for OpenTelemetry trace export, e.g. `http://collector:4317`. |
| `RUST_LOG` | No | `info` | Log level filter. Use `litegen=debug` for verbose logs. |
| `OPENAI_API_KEY` | No | — | OpenAI provider key. |
| `GOOGLE_API_KEY` | No | — | Google (Gemini API) provider key. |
| `STABILITY_API_KEY` | No | — | Stability AI provider key. |
| `BFL_API_KEY` | No | — | Black Forest Labs provider key. |
| `IDEOGRAM_API_KEY` | No | — | Ideogram provider key. |
| `RECRAFT_API_TOKEN` | No | — | Recraft provider token. |
| `PIXELLAB_API_KEY` | No | — | PixelLab provider key. |
| `LEONARDO_API_KEY` | No | — | Leonardo.Ai provider key. |
| `RUNWAY_API_KEY` | No | — | Runway provider key. |
| `LUMA_API_KEY` | No | — | Luma provider key. |
| `KLING_API_KEY` | No | — | Kling AI API key (`Authorization: Bearer`; the access/secret-key pair works only on Kling's legacy API design, which litegen left on 2026-11-03). |
| `MINIMAX_API_KEY` | No | — | MiniMax provider key (`MINIMAX_API_BASE` for the China host). |
| `BYTEDANCE_API_KEY` (or `ARK_API_KEY`) | No | — | BytePlus ModelArk key (`BYTEDANCE_API_BASE` for Volcengine Ark). |
| `TENCENT_SECRET_ID`, `TENCENT_SECRET_KEY` | No | — | Tencent Cloud key pair for Hunyuan (TC3 signing; `TENCENT_REGION`). |
| `TOKENHUB_API_KEY` (or `TOKENHUB_API_KEYS`) | No | — | Tencent TokenHub API key (`Authorization: Bearer`; `TOKENHUB_API_BASE` to override https://tokenhub.tencentmaas.com/v1). |
| `VIDU_API_KEY` | No | — | Vidu provider key. |
| `PIXVERSE_API_KEY` | No | — | PixVerse provider key. |
| `TRIPO_API_KEY` | No | — | Tripo provider key. |
| `MESHY_API_KEY` | No | — | Meshy provider key. |
| `FAL_KEY` | No | — | fal provider key. |
| `REPLICATE_API_TOKEN` | No | — | Replicate provider token. |

Every provider also takes `providers.<p>.api_key` (or a weighted `api_keys` pool) and `api_base`
in `litegen.yaml`, as `LITEGEN__PROVIDERS__<P>__API_KEY` and so on, and an org can store its own
key in the dashboard (Providers). See [docs/provider-config.md](docs/provider-config.md).
For one setting, a `LITEGEN__` variable beats the vendor's own, which beats the file; a blank
vendor variable counts as unset ([docs/configuration.md](docs/configuration.md)).

### Health probes

| Endpoint | Purpose |
|----------|---------|
| `GET /health/live` | Liveness — returns `200 OK {"status":"ok"}` as soon as the process is up. Use as a container liveness probe. |
| `GET /health/ready` | Readiness — returns `200 OK {"status":"ready"}` when the database is reachable, and `503` with `"not_ready"` when it is not. The providers it lists are for information: a deployment whose only keys are its orgs' own is ready too. Use as a container readiness probe before routing traffic. |

### CORS — default deny

LiteGen uses a **default-deny** CORS policy. Browser JavaScript clients will be blocked unless you explicitly set `LITEGEN_CORS_ORIGINS`. In production, set it to your front-end domain:

```bash
LITEGEN_CORS_ORIGINS=https://app.example.com
```

Multiple origins are supported as a comma-separated list.

### Backup

A helper script lives at `scripts/backup-db.sh`. It reads `LITEGEN__DATABASE_URL` from the environment
and writes a timestamped snapshot to a target directory (default: `./backups`).

```bash
# One-off backup (SQLite: the default database, data/litegen.db)
LITEGEN__DATABASE_URL=sqlite://data/litegen.db ./scripts/backup-db.sh /var/backups/litegen

# Postgres
LITEGEN__DATABASE_URL=postgres://user:pass@localhost/litegen ./scripts/backup-db.sh /var/backups/litegen
```

Hourly cron example:

```
# crontab: hourly backup
0 * * * * LITEGEN__DATABASE_URL=... /opt/litegen/scripts/backup-db.sh /var/backups/litegen >> /var/log/litegen-backup.log 2>&1
```

The SQLite path uses `.backup` via the `sqlite3` CLI (safe under concurrent writes). The Postgres path
pipes `pg_dump` through `gzip`. Ship the resulting files to S3, GCS, or any other durable storage.

### Reverse proxy (TLS termination)

Run LiteGen behind **nginx** or **Caddy** for HTTPS. Example Caddyfile:

```
api.example.com {
  reverse_proxy litegen:4000
}
```

LiteGen itself does not terminate TLS. Expose only port 443 publicly; keep port 4000 internal.

### Scaling notes

A single LiteGen instance handles hundreds of concurrent requests. To run several instances behind a plain load balancer, with no sticky sessions, set three things:

- **Postgres** as the database (`LITEGEN_DATABASE_URL`, or the `LITEGEN_DATABASE_*` parts). SQLite allows only one writer at a time.
- **S3** as the file store (`LITEGEN__IMAGE_STORAGE__BACKEND=s3`), so every instance serves the same files.
- **A Redis URL** (`LITEGEN_REDIS_URL`, or `LITEGEN_REDIS_HOST` with its parts). With it, all instances share rate limits, parallel-request caps, the MCP sign-in limit, provider circuit breakers and the image cache, and an admin change reaches every instance within about a second. Without it, those stay in each instance's memory, and a key's limit applies per instance: route each API key to one instance. An admin change still reaches the other instances within 15 seconds.

[docs/scaling.md](docs/scaling.md) lists the connection settings, what the instances share, what happens when Redis is down and how to run Redis. LiteGen logs a startup note when a setting defeats sharing.

## Auth

### First-run flow

When the `users` table is empty, `POST /v1/auth/signup` creates the first account and grants it the **Owner** role automatically.

If `LITEGEN__OWNER_EMAIL` is set, only that email address can claim ownership on first signup — any other email is rejected until an Owner exists to invite them.

**Master-key Bearer auth keeps working as a superuser bypass.** Set `LITEGEN__MASTER_KEY` and include `Authorization: Bearer <value>` on any request to bypass user auth entirely. Useful for bootstrap scripts, CI pipelines, and recovery scenarios.

### Roles

| Role | Description |
|------|-------------|
| **Owner** | Full access including `system:transfer_owner`. Only one Owner at a time. |
| **Admin** | Full access to all resources except transferring ownership. |
| **Member** | Manage own API keys, create generations, view own logs. |
| **Viewer** | Read-only — view own keys and logs; cannot create or mutate. |

### Adding users

Owners and Admins can invite new users from the dashboard:

1. Go to `/users` in the dashboard.
2. Click **Invite**, enter the recipient's email and desired role.
3. An invite link is generated: `https://your-host/invite/<token>`. Copy and send it manually until SMTP is configured.
4. The recipient clicks the link and sets a password, or signs in via OAuth if a provider is pre-configured.

### OAuth setup

Set the following environment variables to enable GitHub and/or Google OAuth:

```bash
LITEGEN__OAUTH__GITHUB__CLIENT_ID=...
LITEGEN__OAUTH__GITHUB__CLIENT_SECRET=...
LITEGEN__OAUTH__GOOGLE__CLIENT_ID=...
LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET=...
LITEGEN__OAUTH__CALLBACK_BASE=https://your-litegen.example.com
```

Register these callback URLs with your OAuth provider:

- **GitHub:** `${CALLBACK_BASE}/v1/auth/oauth/github/callback`
- **Google:** `${CALLBACK_BASE}/v1/auth/oauth/google/callback`

**Important:** OAuth does not auto-create accounts. A user must already be invited (their email must exist in the `users` table) before OAuth sign-in will succeed. Attempting to OAuth with an uninvited email returns `account_not_invited`.

### Session cookies

| Cookie | Description |
|--------|-------------|
| `litegen_session` | HttpOnly + Secure + SameSite=Lax, 7-day TTL with sliding expiry. |
| `litegen_csrf` | Readable CSRF token — include as `X-CSRF-Token` on any mutating session-authed request. |

For local HTTP development (without HTTPS), set `LITEGEN__COOKIE_INSECURE_DEV=true` to allow cookies over plain HTTP. Do not use this in production.

### API keys vs sessions

LiteGen supports two independent authentication modes:

- **Bearer API keys** — programmatic clients. Each key carries scopes (`generate`, `read`, `admin`), budget caps, and RPM limits. Pass as `Authorization: Bearer <key>`.
- **Session cookies** — human dashboard users. Roles and permissions are enforced on every request.
- **Master key** — set via `LITEGEN__MASTER_KEY`. Bypasses both modes; treated as a superuser for all endpoints.

Both modes can be active simultaneously. Programmatic clients continue working unchanged when user auth is enabled.

## Multi-tenant hosting

LiteGen runs in one of two modes, selected by `LITEGEN__MODE`:

```bash
LITEGEN__MODE=single_tenant   # default — unchanged single-deployment behavior
LITEGEN__MODE=hosted          # multi-tenant SaaS: open self-serve signup
```

The default is `single_tenant`; everything documented above (first-run owner signup, master-key bypass, roles, sessions) behaves exactly as before. **Hosted mode** opts into the multi-tenant platform:

- `POST /v1/auth/signup` becomes **open self-serve signup**. Each new user gets their own **Organization** (plus a default Application) and becomes its Owner — signup is no longer gated by the empty-`users` check or `LITEGEN__OWNER_EMAIL`.
- The master key is **demoted to platform-admin**: it is still a superuser for operational endpoints, but it carries **no implicit tenant access** — it is not a member of any org and cannot read tenant-scoped data by default.

### The tenancy model

```
User ──< OrganizationMember >── Organization ──< Application ──< API key
                (per-org role)
```

- A **User** belongs to one or more **Organizations** via memberships, each carrying a per-org **role**: `owner`, `admin`, `member`, or `viewer`.
- An **Organization** owns one or more **Applications**.
- An **Application** owns its **API keys** and its own upstream provider credentials.

### API keys are id + secret

Keys are minted as an id/secret pair:

- A **public id** `pk_live_…` — safe to display; shown in the dashboard key list.
- A **secret** `sk_live_…` — shown **once** at creation (and on rotation), never again.

Programmatic clients send the secret as `Authorization: Bearer sk_live_…`; the secret resolves the owning tenant (org + app) server-side. This is a drop-in for OpenAI-style SDKs — point the SDK's base URL at your LiteGen deployment and use the `sk_live_…` secret as the API key. Requests are strict by default: a field the model passes through (its `extra_allowlist`, such as `user` and `moderation` on the OpenAI image models) is forwarded to the vendor, at the top level as in `extra`; any other field the request has no such name for is refused with 400 `param_unsupported`, unless the request sends `strict: false`, which drops it and names it in `X-Litegen-Dropped-Params` ([Strict requests](#strict-requests)).

Rotation re-issues a new `pk_live_…`/`sk_live_…` pair **in place** on the same key record (preserving its name, scopes, quota, and RPM limit); the old secret stops working immediately.

A key acts for one person: whoever made it or, after a rotation, whoever rotated it. It stops working when that person leaves its org, and holds `admin` only while their role could grant it. Keys made or rotated with the master key belong to the deployment. See "A key stopped working after someone left" in `docs/RUNBOOK.md`.

### Dashboard org/app scoping

A dashboard session can belong to multiple orgs and apps. The active selection is sent on each request as:

- `X-Litegen-Org-Id` — the active organization.
- `X-Litegen-App-Id` — the active application within that org.

Both headers are validated against the user's memberships server-side, so a session can never read or mutate a tenant it isn't a member of.

### BYO provider keys

Each Application stores its **own** upstream provider credentials, encrypted at rest with **AES-256-GCM**. To enable credential storage in hosted mode, provide a base64-encoded 32-byte key:

```bash
LITEGEN__SECRETS_KEY=$(head -c 32 /dev/urandom | base64)
```

At generation time (and during background video-status polling) each request is dispatched using that app's decrypted provider credentials, keeping tenants' upstream keys isolated. Resolution order per provider is: **the app's stored credential → the global env-configured credential (if any) → `400 provider_not_configured`** when neither exists. Cost-estimate endpoints follow the same rule. Decryption/parse failures surface as `500` (server misconfiguration), never a silent fallback to another key.

### BYO object storage (per app)

Each application can store its generated images in its own S3-compatible bucket
(AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces). Configure it per app:

- `GET /v1/apps/{app_id}/storage` — current config (never returns the secret)
- `PUT /v1/apps/{app_id}/storage` — set/update; body: `bucket_name` (required),
  `region`, `endpoint_url`, `custom_public_url`, `path_prefix`, and either the
  write-only `access_key_id` + `secret_access_key` (send both to set/rotate; omit
  both to keep the existing pair) or `credential_mode: "assume_role"` with a
  `role_arn`: a cross-account IAM role litegen assumes with the app's
  `external_id` (shown by the GET). `fallback_to_litegen_store: false` keeps the
  app's files out of litegen's own store while the bucket can't be reached.
- `DELETE /v1/apps/{app_id}/storage` — revert to the platform default store

The credential pair is encrypted at rest with AES-256-GCM using
`LITEGEN__SECRETS_KEY` (required in hosted mode). Apps with no storage configured
fall back to the global `image_storage` backend (S3 if set, else base64 inline).
Whether outputs are kept at all (`keep_outputs: false` keeps nothing and hands out
the vendor's own links), for how long (`retention_days`), and who may open them is
the app's storage settings ([docs/storage-access.md](docs/storage-access.md)). For
`signed` and `private`, keep the bucket itself private.

### Scaling roadmap

Horizontal multi-instance scaling is done (**Phase 3**): with Postgres, S3 and a Redis URL, instances share rate limits, circuit-breaker state, the image cache and admin changes (see [Scaling notes](#scaling-notes) and [docs/scaling.md](docs/scaling.md)). Automated infrastructure provisioning is still to come (**Phase 4**).

## License

MIT
