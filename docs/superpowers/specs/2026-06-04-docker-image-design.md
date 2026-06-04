# LiteGen Production Docker Image — Design

- **Date:** 2026-06-04
- **Status:** Approved (pending spec review)
- **Topic:** Publish a production-grade, LTS, multi-arch Docker image for LiteGen that users can pull, run, and configure easily — bundling both the API/proxy and the dashboard UI in a single image.

## 1. Context & current state

LiteGen is a Rust (axum) proxy for AI image/video generation. Today:

- **`Dockerfile`** (repo root): solid multi-stage build (`rust:1.94-slim` builder → `debian:trixie-slim` runtime), non-root `USER 1000:1000`, `EXPOSE 4000`, copies the `litegen` binary + `models/`. **No published image, no version label, no OCI metadata, no in-image HEALTHCHECK.**
- **`docker-compose.yml`** (repo root): uses `build: .` (forces a local clone+build), defaults to Postgres, requires `LITEGEN_MASTER_KEY`. The README Quick Start tells users to `git clone` then `docker compose up -d` — **there is no image to pull.**
- **Database:** `sqlx` is compiled with both `sqlite` and `postgres` features; migrations exist for both (`litegen-core/migrations/{sqlite,postgres}`). The config default is `database_url = "sqlite://litegen.db"` (`litegen-core/src/config/mod.rs:130`) and `master_key` is `Option<String>` defaulting to `None` (boots without auth). SQLite migrations run automatically on connect (`litegen-core/src/db/sqlite.rs:38`).
- **SQLite file creation:** nothing in the code sets `create_if_missing` / `?mode=rwc` (verified by grep). A first run against a non-existent file needs the URL to carry `?mode=rwc`.
- **Dashboard:** a separate Vite SPA in `dashboard/` (build: `tsc -b && vite build` → `dashboard/dist`). It depends on the local SDK `@litegen/sdk` via `file:../sdks/typescript`. Its API base is a compile-time Vite define `__LITEGEN_API_BASE__` (`dashboard/vite.config.ts:14`); the SDK then calls `${BASE}/v1/...`. In the hosted deployment Caddy serves the SPA and reverse-proxies `/api` → API; the Rust binary itself does **not** serve static assets (`tower-http` has no `fs` feature today).
- **Release CI:** none for Docker (only `.github/workflows/pages.yml`).
- **Git remote:** `visgotti/litegen-first`. `Cargo.toml` aspirationally points `repository` at `github.com/litegen/litegen`.

## 2. Goals / non-goals

**Goals**

1. A single, production-grade image that serves **both** the API/proxy **and** the dashboard UI on one port.
2. **Easy to pull:** published to Docker Hub **and** GHCR, multi-arch (amd64 + arm64).
3. **Easy to use:** a zero-dependency one-liner using embedded SQLite that yields a working API + UI in ~30 seconds; a pull-based Postgres compose for scale.
4. **Easy to configure:** documented env-var surface, sane defaults, a single persistent volume.
5. **LTS versioning:** immutable `X.Y.Z` tags plus floating `X.Y`, `X`, and `latest`, published from git tags via CI, with SBOM + provenance.

**Non-goals**

- The hosted multi-tenant / OAuth platform configuration (already handled by `deploy.js` + `deploy/`). The public image defaults to **single-tenant proxy mode**.
- An entrypoint script for master-key auto-generation (decided against — see §3).
- Changing the existing `deploy.js` droplet pipeline (it keeps building its own private GHCR image; this work is additive).

## 3. Decisions (from brainstorming)

| Decision | Choice |
|---|---|
| Registry | **Docker Hub + GHCR (both)** |
| Headline quickstart | **SQLite one-liner first**, Postgres compose second |
| Versioning / LTS | **Semver + floating majors (`X`, `X.Y`) + `latest`**, published on git tag `v*.*.*` |
| Architectures | **amd64 + arm64** |
| Auth on zero-config run | **Open by default, loud log warning** to set `LITEGEN__MASTER_KEY` (no entrypoint script) |
| Dashboard | **Bundle into the image** (one `docker run` serves API + UI) |

## 4. Architecture

### 4.1 Three-stage `Dockerfile`

**Stage 1 — dashboard build (`node:22-slim`)**
- Copy `sdks/typescript/` and `dashboard/` into the build context.
- `npm ci` (resolves the `file:../sdks/typescript` dependency); build the SDK if it needs a build step, then run the dashboard build.
- Build with the API base define set to **same-origin root** (empty string) so the SPA calls `/v1/...` on its own origin — no Caddy/reverse-proxy needed. The mechanism is whatever env var `dashboard/vite.config.ts` reads into `API_BASE` (set it to `""`).
- Output: `dashboard/dist`.

**Stage 2 — Rust build (`rust:1.94-slim`)**
- Unchanged from today: build deps (`pkg-config libssl-dev ca-certificates`), copy `litegen-core/{Cargo.toml,Cargo.lock,src,migrations}` + `models`, `cargo build --release --bin litegen`.
- (Optional, flagged) cargo-chef layer caching for faster rebuilds — nice-to-have, not required for correctness.

**Stage 3 — runtime (`debian:trixie-slim`)** — must stay on the builder's Debian release (trixie) for glibc compatibility.
- Install `ca-certificates sqlite3 wget tini`.
- Copy `--from=rust` the `litegen` binary; copy `models/`; copy `--from=dashboard` the built dashboard into `/app/dashboard`.
- **OCI labels** from build args: `org.opencontainers.image.{title,description,source,version,revision,licenses,created}`.
- Bake version: `ARG VERSION` → `ENV LITEGEN_VERSION=${VERSION}`.
- **In-image `HEALTHCHECK`**: `wget -qO- http://127.0.0.1:4000/health/live || exit 1`.
- **`tini` as PID 1** (`ENTRYPOINT ["/usr/bin/tini","--","/app/litegen"]`) for signal handling / zombie reaping.
- **Persistence:** `VOLUME /data`; default env:
  - `LITEGEN__SERVER__HOST=0.0.0.0`, `LITEGEN__SERVER__PORT=4000`
  - `LITEGEN__DATABASE_URL=sqlite:///data/litegen.db?mode=rwc` (three slashes = absolute path `/data/litegen.db` under the volume; `?mode=rwc` guarantees first-run file creation — implementation must verify the exact slash form sqlx accepts for an absolute path)
  - local image-storage path under `/data` (so generated artifacts persist on the same volume)
  - `LITEGEN_MODELS_DIR=/app/models`, `LITEGEN_DASHBOARD_DIR=/app/dashboard`
- Non-root `USER 1000:1000`; ensure `/data` is writable by that uid (create + `chown` at build time; the volume inherits the mountpoint owner).
- `EXPOSE 4000`.

### 4.2 Rust change — serve the dashboard (minimal)

- Add the **`fs`** feature to `tower-http` in `litegen-core/Cargo.toml`.
- In the router assembly (`api::create_router`, mounted in `litegen-core/src/main.rs:232`), add a **fallback service** that serves the bundled SPA:
  - `ServeDir::new(dashboard_dir).fallback(ServeFile::new(dashboard_dir/index.html))` so client-side routes resolve to `index.html`.
  - Mount as `.fallback_service(...)` so it only handles requests that match **no** API route. Existing routes (`/v1/*`, `/health/*`, `/metrics`, auth endpoints) keep precedence; `/`, `/assets/*`, `/login`, etc. serve the SPA.
  - **Gate on existence:** read `LITEGEN_DASHBOARD_DIR`; only attach the fallback if the directory exists. Non-Docker runs (dev, `cargo run`) where the dir is absent behave exactly as today.
- No change to the dashboard's API-base logic at runtime — same-origin is achieved purely by the build-time define in Stage 1.

### 4.3 Auth behavior

- Keep `master_key: Option<String>` (open when unset).
- On startup, if no master key is configured **and** mode is single-tenant, log a prominent `WARN` (e.g. "No LITEGEN__MASTER_KEY set — the API is unauthenticated; set one before exposing this to a network."). This is a small, localized addition to the startup path in `main.rs`; no entrypoint script.

### 4.4 Quickstart UX

**Headline one-liner (SQLite, zero deps):**
```bash
docker run -d --name litegen -p 4000:4000 -v litegen:/data \
  -e OPENAI_API_KEY=sk-... \
  litegen/litegen:1
```
→ API at `http://localhost:4000/v1/...`, dashboard at `http://localhost:4000`, health at `/health/live`. Data persists in the `litegen` volume.

**Production compose (Postgres):** rewrite the root `docker-compose.yml` to **pull** `litegen/litegen:1` (drop `build: .`), keep the Postgres service + healthchecks, point `LITEGEN__DATABASE_URL` at the `db` service. Ship a `.env.example`. Move the existing build-from-source compose to `docker-compose.build.yml` for contributors.

### 4.5 Versioning / LTS + release CI

New workflow `.github/workflows/docker-publish.yml`:
- **Triggers:** push of tag `v*.*.*`; plus `workflow_dispatch` for manual re-runs.
- **Build:** `docker/setup-qemu-action` + `docker/setup-buildx-action`; `docker/build-push-action` with `platforms: linux/amd64,linux/arm64`, build args (`VERSION`, `REVISION`, `BUILD_DATE`), `sbom: true`, `provenance: true`.
- **Tags (via `docker/metadata-action`):** for tag `v1.4.2` → `1.4.2`, `1.4`, `1`, `latest`. Pushed to **both** `docker.io/<NS>/litegen` and `ghcr.io/<owner>/litegen`.
- **Auth:** Docker Hub via repo secrets `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN`; GHCR via the built-in `GITHUB_TOKEN` (`packages: write`).
- **Image namespace is a single workflow variable** so it can be flipped between `litegen/litegen` and `visgotti/litegen` without touching anything else.
- The user adds the secrets and pushes a tag to perform the first real publish; CI is otherwise complete.

### 4.6 Docs

- Refresh README **Quick Start**: the one-liner first, then the Postgres compose; replace the clone-and-build instructions as the primary path (keep build-from-source as a secondary section).
- Add a complete **env-var reference table**: `LITEGEN__SERVER__*`, `LITEGEN__DATABASE_URL`, `LITEGEN__MASTER_KEY`, `LITEGEN__MODE`, `LITEGEN__IMAGE_STORAGE__*` / S3 vars, provider keys, `LITEGEN_CORS_ORIGINS`, `RUST_LOG`, plus `LITEGEN_VERSION` / `LITEGEN_DASHBOARD_DIR` / `LITEGEN_MODELS_DIR`.
- Document **persistence** (`/data` volume), **upgrade/pinning** (pin `:1` for auto-patch within a stable major = the LTS contract; `:X.Y.Z` for fully reproducible), and the **open-by-default auth** caveat.
- Add `docker/README.md` as the Docker Hub repository description (overview + one-liner + tags + config table).

## 5. File-change summary

| File | Change |
|---|---|
| `Dockerfile` | Add dashboard build stage; runtime stage gains OCI labels, version arg, tini, HEALTHCHECK, `/data` volume + SQLite default, dashboard copy. |
| `litegen-core/Cargo.toml` | Add `fs` to `tower-http` features. |
| `litegen-core/src/main.rs` (+ `api/mod.rs` as needed) | Conditional SPA `fallback_service`; startup WARN when no master key in single-tenant. |
| `dashboard/vite.config.ts` | Ensure API-base define resolves to `""` (same-origin) when built for the bundled image (driven by a build env var). |
| `docker-compose.yml` | Rewrite to pull `litegen/litegen:1` + Postgres; add `.env.example`. |
| `docker-compose.build.yml` | New — the previous build-from-source compose. |
| `.github/workflows/docker-publish.yml` | New — multi-arch tag-driven publish to Hub + GHCR with SBOM/provenance. |
| `README.md` | Rewritten Quick Start + env reference + persistence/upgrade/auth docs. |
| `docker/README.md` | New — Docker Hub description. |

## 6. Testing / verification

1. **Local multi-arch build:** `docker buildx build --platform linux/amd64,linux/arm64 ...` succeeds; image size is reasonable.
2. **SQLite one-liner:** `docker run` with a fresh volume → `/health/live` returns OK; `GET /` serves the dashboard `index.html`; a deep link (e.g. `/some/spa/route`) also returns `index.html`; `GET /v1/...` hits the API (not the SPA); the SQLite file is created in the volume and survives a container restart.
3. **No master key:** the startup WARN is emitted.
4. **Postgres compose:** `docker compose up -d` (pulling the published image) → both services healthy, API + dashboard reachable.
5. **Dashboard → API same-origin:** the SPA's network calls go to `/v1/...` on the same origin and succeed (no localhost/`/api` base leaked into the bundle).
6. **Non-Docker run unaffected:** `cargo run` with no `LITEGEN_DASHBOARD_DIR` behaves exactly as before (no fallback attached).
7. **CI dry run:** the publish workflow builds on a test tag and produces correctly-tagged multi-arch manifests with SBOM/provenance.

## 7. Risks / open items

- **Docker Hub namespace `litegen`:** bare `litegen` was already unavailable on npm (`npm-sdk` memory); `litegen/litegen` on Docker Hub may likewise be taken. Mitigation: namespace is a single CI/build variable; fall back to `visgotti/litegen`. **Confirm ownership before first publish.**
- **arm64 cross-build time:** QEMU-emulated arm64 Rust release builds are slow in CI. Acceptable for tag-driven releases; revisit native arm64 runners if it becomes painful.
- **Image size:** bundling the dashboard + `models/` grows the image. Keep the runtime stage slim (only runtime deps); the dashboard `dist` is small.
- **SQLite `?mode=rwc`:** relies on sqlx honoring the query param for file creation; verified as the supported mechanism. If a future code change adds `create_if_missing(true)` in `db/sqlite.rs`, the URL param becomes redundant (harmless).
