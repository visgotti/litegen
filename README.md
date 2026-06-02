📖 **Docs, SDKs & API reference → [visgotti.github.io/litegen](https://visgotti.github.io/litegen/)**

# LiteGen

> The universal proxy for AI image and video generation. Like LiteLLM, but for multimedia.

LiteGen provides a unified API gateway for all major image and video generation providers. Route requests across OpenAI DALL-E, Stability AI, Replicate, Google Imagen, Fal.ai, Runway, Luma, and more — with automatic fallback, weighted load balancing, caching, cost tracking, and a real-time dashboard.

## Features

- **Unified API** — OpenAI-compatible REST endpoints for image & video generation
- **10+ Providers** — OpenAI, Stability, Replicate, Google, Fal, Runway, Luma (more coming)
- **Smart Routing** — Fallback chains, weighted round-robin, lowest-cost, lowest-latency
- **Caching** — In-memory cache with configurable TTL to avoid duplicate generations
- **Cost Tracking** — Per-request cost estimation and aggregate spend analytics
- **API Key Management** — Create/revoke keys, weighted key pools for rate limit distribution
- **Observability** — Request logging, Prometheus metrics, structured tracing
- **React Dashboard** — Real-time monitoring, provider health, request logs, cost charts
- **Easy Config** — YAML config + env vars, auto-discovers provider keys
- **Self-Hosted** — Single binary + Docker, SQLite or Postgres

## What's new in v0.2

- **Capability registry with per-model schemas** — YAML-driven model declarations define allowed parameters, sizes, aspect ratios, and reference-image roles; unknown params are rejected (strict mode) or silently dropped (lax mode).
- **`reference_images` tagged-union** — Send reference images as URLs, base64 blobs, or multipart file uploads using `{type: "url"|"base64"|"blob", value: "..."}`.
- **Strict validation with per-request lax opt-out** — Default `strict: true` rejects unsupported parameters; set `strict: false` to drop unknowns and get an `X-Litegen-Dropped-Params` response header.
- **DB-backed multi-key auth: scopes, quotas, RPM rate limits** — Create API keys with per-key USD budget caps, requests-per-minute throttles, and CSV scope sets (`generate`, `read`, `admin`).
- **DB-backed video generations + polling endpoint** — Video jobs are persisted to the `generations` table; poll status via `GET /v1/generations/{id}`. A background poller updates status every 5 s.
- **Per-key webhook delivery on video completion** — When a generation reaches a terminal status (`completed`, `failed`, `cancelled`), the owning key's `webhook_url` receives a signed `POST` with the generation payload (`X-Litegen-Signature: sha256=<hex>`). Retries 3 times with exponential back-off.
- **OpenTelemetry OTLP gRPC export** — Set `OTEL_EXPORTER_OTLP_ENDPOINT` to ship traces to any OTel collector.
- **Prometheus `/metrics`** — Expose Prometheus-format metrics at `GET /metrics`.
- **Dashboard CRUD UI for keys + model schema panel** — Create, update, and revoke API keys from the React dashboard; inspect full model capability schemas in the Models panel.

## Running locally

The minimum required env vars:

```bash
# Port the proxy listens on (default: 4000)
export LITEGEN__SERVER__PORT=4000

# Master API key — if set, all requests need Authorization: Bearer <key>
# Omit to run in dev mode (no auth)
export LITEGEN__MASTER_KEY=your-secret-master-key

# Database URL (SQLite for dev, Postgres for prod)
export LITEGEN__DATABASE_URL=sqlite://litegen.db

# Directory containing provider YAML model definitions (default: ./models)
export LITEGEN_MODELS_DIR=./models

# Provider API keys (as needed)
export OPENAI_API_KEY=sk-...
export RUNWAY_API_KEY=...
```

Then start the backend and dashboard:

```bash
# Backend
cd litegen-core
cargo build --release
./target/release/litegen

# Dashboard (separate terminal)
cd dashboard
npm install
npm run dev
```

## Quick Start

### Docker (recommended)

```bash
# Clone and configure
git clone https://github.com/litegen/litegen.git
cd litegen
cp litegen.example.yaml litegen.yaml
# Edit litegen.yaml with your API keys

# Run
docker compose up -d
```

Open http://localhost:4000/health to verify, and http://localhost:5173 for the dashboard.

### From Source

```bash
# Backend
cd litegen-core
cargo build --release
./target/release/litegen

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

## API Reference

### Generate Image
```bash
curl -X POST http://localhost:4000/v1/images/generations \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "a serene mountain landscape at sunset",
    "model": "openai/dall-e-3",
    "size": "1024x1024",
    "quality": "hd"
  }'
```

### Generate Video
```bash
curl -X POST http://localhost:4000/v1/videos/generations \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "a timelapse of clouds over a city",
    "model": "runway/gen-3",
    "duration_seconds": 5
  }'
```

### List Models
```bash
curl http://localhost:4000/v1/models
```

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

## Model Routing

Configure sophisticated routing in `litegen.yaml`:

```yaml
model_routes:
  # Fallback: try OpenAI first, then Stability
  - model: "dall-e-3"
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
                     │  │(SQLx)│  │(Prom/OTel│  │ OpenAI   │   │
                     │  └──────┘  └──────────┘  │ Stability│   │
                     │                          │ Replicate│   │
                     │  ┌────────────────────┐  │ Google   │   │
                     │  │  React Dashboard   │  │ Fal      │   │
                     │  │  (Vite + Recharts) │  │ Runway   │   │
                     │  └────────────────────┘  │ Luma     │   │
                     │                          └──────────┘   │
                     └──────────────────────────────────────────┘
```

## Project Structure

```
litegen/
├── litegen-core/          # Rust backend
│   ├── src/
│   │   ├── main.rs        # Server entrypoint
│   │   ├── lib.rs         # Library exports
│   │   ├── api/           # REST API handlers + auth middleware
│   │   ├── config/        # YAML + env config loading
│   │   ├── db/            # SQLite/Postgres persistence
│   │   ├── providers/     # Provider implementations
│   │   │   ├── image/     # OpenAI, Stability, Replicate, Google, Fal, Mock
│   │   │   └── video/     # OpenAI (Sora), Fal, Replicate, Runway, Luma, Mock
│   │   ├── proxy/         # Router, registry, cache
│   │   └── types/         # Shared types + OpenAPI schemas
│   ├── migrations/        # SQL migrations
│   └── tests/             # Integration tests
├── dashboard/             # React + Vite dashboard
│   └── src/pages/         # Overview, Logs, Models, Health, Keys
├── Dockerfile             # Multi-stage build
├── docker-compose.yml     # One-command deployment
└── litegen.example.yaml   # Example configuration
```

## Supported Providers

Several vendors serve **both** image and video under one provider name — e.g. a single
`openai` provider routes `openai/dall-e-3` (image) and `openai/sora` (video), and the same
holds for `fal` and `replicate`. The model ID prefix (`<provider>/<model>`) selects the
modality automatically.

| Provider | Image | Video | Models |
|----------|-------|-------|--------|
| OpenAI | ✅ | ✅ | DALL-E 2/3 · **Sora 2 / Sora 2 Pro** |
| Fal.ai | ✅ | ✅ | Flux, SDXL, Recraft · Kling, MiniMax, SVD, LTX |
| Replicate | ✅ | ✅ | Flux, SDXL, SD3 · AnimateDiff, SVD, Zeroscope |
| Google | ✅ | 🔜 | Imagen 3, Gemini 2.5/3 image · _Veo 3 video — public API, not yet wired_ |
| Stability AI | ✅ | — | SD3.5, Core, Ultra, SDXL · _(Stable Video hosted API discontinued)_ |
| Runway | 🔜 | ✅ | _Gen-4 Image — public API, not yet wired_ · Gen-3 / Gen-4 video |
| Luma | 🔜 | ✅ | _Photon image — public API, not yet wired_ · Dream Machine / Ray video |

**Legend:** ✅ supported in litegen · 🔜 vendor offers a public API for this modality but litegen
hasn't wired it yet (see [Roadmap: missing providers](#roadmap--missing-providers)) · — no public API.

## Roadmap — Missing Providers

Gaps below are catalogued from a May 2026 audit of official vendor APIs. "Endpoint" is the
upstream create/generate call a litegen provider would target.

### Missing modalities for providers we already have (public API exists)

| Gap | Modality | Upstream endpoint |
|-----|----------|-------------------|
| **Google Veo** (biggest gap — Google is image-only today) | video | `POST generativelanguage.googleapis.com/v1beta/models/{veo-3.x}:predictLongRunning` |
| **Luma Photon** (Luma is video-only today) | image | `POST api.lumalabs.ai/dream-machine/v1/generations/image` (photon-1, photon-flash-1) |
| **Runway image** (Runway is video-only today) | image | `POST api.dev.runwayml.com/v1/text_to_image` (gen4_image, gen4_image_turbo) |

> _Stability **Stable Video** is intentionally omitted — Stability discontinued the hosted
> video API (self-host only), so there is no public endpoint to wire._

### Missing vendors entirely (have a public developer API)

**Both image + video:**
- **Amazon Bedrock** — Nova Canvas / Titan (image) + Nova Reel (video). `bedrock-runtime.{region}.amazonaws.com` (SigV4 auth).
- **MiniMax / Hailuo** — `image-01` (image) + Hailuo (video). `api.minimax.io/v1/{image_generation,video_generation}`.
- **Kling AI (Kuaishou)** — kling image + video. `api.klingai.com/v1/{images/generations,videos/text2video}`.
- **Leonardo.Ai** — Phoenix (image) + Motion (video). `cloud.leonardo.ai/api/rest/v1`.
- **ByteDance Seedance** (BytePlus ModelArk / Volcengine Ark) — Seedream (image) + Seedance (video).
- **Tencent Hunyuan** — image + video. `aiart.tencentcloudapi.com`.

**Image only:**
- **Black Forest Labs (FLUX)** — `api.bfl.ai/v1/{model}` (flux-2-pro, flux-pro-1.1, flux-kontext). Direct FLUX, no Replicate/Fal hop.
- **Ideogram** — `api.ideogram.ai/v1/ideogram-v3/generate` (strong text rendering).
- **Recraft** — `external.api.recraft.ai/v1/images/generations` (vector/brand styles).

**Video only:**
- **Vidu** — `api.vidu.com/ent/v2/{text2video,img2video}`.
- **Pixverse** — public API (also reachable via Fal).

**No first-party public API (reachable only indirectly / not integrable directly):**
- **Midjourney** — image + video, but UI/Discord only; no REST API.
- **Pika** — video only; no first-party API (exposed via the Fal aggregator).
- **Adobe Firefly** — image + video via *Firefly Services* (enterprise/Adobe IMS auth), not a simple key.

## Deployment

### Quick start with docker compose

```bash
LITEGEN_MASTER_KEY=your-secret-key docker compose up -d
```

The service starts on port **4000**. The Postgres database is provisioned automatically; no manual migration step is needed.

To allow browser clients through CORS:

```bash
LITEGEN_MASTER_KEY=your-secret-key LITEGEN_CORS_ORIGINS=https://your-domain docker compose up -d
```

### Environment variable reference

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LITEGEN__MASTER_KEY` | Yes | — | Bearer token that grants full admin access. All API requests must include `Authorization: Bearer <value>`. |
| `LITEGEN__DATABASE_URL` | No | `sqlite://litegen.db` | SQLite (`sqlite://path/to/file.db`) or Postgres (`postgres://user:pass@host/db`) connection URL. |
| `LITEGEN__SERVER__HOST` | No | `127.0.0.1` | Bind address. Set to `0.0.0.0` in containers. |
| `LITEGEN__SERVER__PORT` | No | `4000` | TCP port the HTTP server listens on. |
| `LITEGEN_MODELS_DIR` | No | `./models` | Directory containing provider YAML model capability definitions. |
| `LITEGEN_CORS_ORIGINS` | No | _(deny all)_ | Comma-separated list of allowed CORS origins, e.g. `https://app.example.com`. Leave empty to deny browser cross-origin requests. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | No | — | gRPC endpoint for OpenTelemetry trace export, e.g. `http://collector:4317`. |
| `RUST_LOG` | No | `info` | Log level filter. Use `litegen=debug` for verbose logs. |
| `OPENAI_API_KEY` | No | — | OpenAI provider key. |
| `STABILITY_API_KEY` | No | — | Stability AI provider key. |
| `REPLICATE_API_TOKEN` | No | — | Replicate provider token. |
| `GOOGLE_API_KEY` | No | — | Google Imagen provider key. |
| `FAL_KEY` | No | — | Fal.ai provider key. |
| `RUNWAY_API_KEY` | No | — | Runway provider key. |
| `LUMA_API_KEY` | No | — | Luma Dream Machine provider key. |

### Health probes

| Endpoint | Purpose |
|----------|---------|
| `GET /health/live` | Liveness — returns `200 OK {"status":"alive"}` as soon as the process is up. Use as a container liveness probe. |
| `GET /health/ready` | Readiness — returns `200 OK {"status":"ready"}` when the database is reachable and at least one provider is configured. Returns `503` otherwise. Use as a container readiness probe before routing traffic. |

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
# One-off backup
LITEGEN__DATABASE_URL=sqlite://litegen.db ./scripts/backup-db.sh /var/backups/litegen

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

A single LiteGen instance handles hundreds of concurrent requests. When scaling to multiple instances, be aware:

- **Rate limiting** and **circuit breakers** are currently tracked in-process memory. A request hitting instance A does not share state with instance B. For multi-instance deployments, route a given API key to the same instance (sticky sessions by `Authorization` header) until a Redis-backed shared state layer is added in a future release.
- **Postgres** is recommended over SQLite for multi-instance deployments (SQLite allows only one writer at a time).

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

## License

MIT
