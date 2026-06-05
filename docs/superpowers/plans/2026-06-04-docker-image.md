# LiteGen Production Docker Image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a single production-grade, multi-arch Docker image that serves both the LiteGen API/proxy and the dashboard UI on one port, published to Docker Hub + GHCR from git tags, runnable as a zero-dependency SQLite one-liner.

**Architecture:** A three-stage Docker build (Node builds the dashboard SPA → Rust builds the `litegen` binary → slim Debian runtime bundles both). The Rust server gains an optional static-file fallback (`tower-http` `ServeDir`) that serves the bundled SPA only when `LITEGEN_DASHBOARD_DIR` points at an existing directory, so non-Docker runs are unchanged. A tag-driven GitHub Actions workflow builds amd64+arm64 and pushes semver/floating/`latest` tags with SBOM + provenance.

**Tech Stack:** Rust (axum 0.8, tower-http 0.6), Vite/React dashboard, `@litegen/sdk` (tsup), Docker buildx, GitHub Actions, SQLite/Postgres via sqlx.

**Reference spec:** `docs/superpowers/specs/2026-06-04-docker-image-design.md`

---

## Key facts the implementer must know

- **No Cargo workspace at repo root.** Build/test the Rust crate from `litegen-core/` (`cd litegen-core && cargo test ...`). Bin + lib are both named `litegen`.
- **Dashboard API base is build-time.** `dashboard/vite.config.ts` computes `API_BASE = (process.env.VITE_API_URL || 'http://localhost:4000').replace(/\/+$/, '')` and injects it as the `__LITEGEN_API_BASE__` define. An **empty** `VITE_API_URL` falls back to localhost; pass **`VITE_API_URL=/`** → `API_BASE === ''` → the SPA calls `/v1/...` on its own origin. Do **not** pass an empty string.
- **The SDK must be built before the dashboard.** `dashboard/package.json` depends on `@litegen/sdk` via `file:../sdks/typescript`; that package has **no** `prepare` script and its `dist/` is gitignored. Build it explicitly (`npm ci && npm run build`, which runs `tsup`) before building the dashboard.
- **`LocalStorage` is a no-op** (`litegen-core/src/proxy/storage.rs:159`): with the default `local` image-storage backend, generated images are not written to disk. The `/data` volume primarily persists the **SQLite database**. Real artifact persistence uses the S3 backend. Do not promise local image persistence in docs.
- **SQLite file creation** needs `?mode=rwc` in the URL (nothing in code sets `create_if_missing`). The runtime image sets `LITEGEN__DATABASE_URL=sqlite:///data/litegen.db?mode=rwc`.
- **Router final assembly** is in `litegen-core/src/api/handlers/mod.rs:1846-1857` (`create_router` ends with `.with_state(state)`, returning `axum::Router` i.e. `Router<()>`). The dashboard fallback is attached to that returned router (state already erased), so it lives in `main.rs`, not inside `create_router`.
- **`Mode`** enum: `Mode::SingleTenant` (default) and `Mode::Hosted` (`litegen-core/src/config/mod.rs:9-13`).

---

## File structure

| File | Responsibility | Action |
|---|---|---|
| `litegen-core/Cargo.toml` | Add `fs` to `tower-http` features | Modify (`:23`) |
| `litegen-core/src/api/mod.rs` | New `attach_dashboard(router, dir)` helper + its tests | Modify |
| `litegen-core/src/config/mod.rs` | New `warn_open_auth(mode, has_master_key)` predicate + test | Modify |
| `litegen-core/src/main.rs` | Wire dashboard fallback (conditional) + emit open-auth warning | Modify (`:55`, `:232-234`) |
| `Dockerfile` | Three-stage build: dashboard → rust → runtime | Rewrite |
| `.dockerignore` | Keep build context small | Create |
| `docker-compose.yml` | Pull published image + Postgres | Rewrite |
| `docker-compose.build.yml` | Build-from-source compose (old behavior) | Create |
| `.env.example` | Compose env template | Create |
| `.github/workflows/docker-publish.yml` | Tag-driven multi-arch publish | Create |
| `docker/README.md` | Docker Hub repository description | Create |
| `README.md` | Quick Start (one-liner first), env reference, persistence/auth | Modify |

---

## Task 1: Rust — `attach_dashboard` static-file fallback helper

**Files:**
- Modify: `litegen-core/Cargo.toml:23`
- Modify: `litegen-core/src/api/mod.rs`
- Test: inline `#[cfg(test)]` module in `litegen-core/src/api/mod.rs`

- [ ] **Step 1: Enable the tower-http `fs` feature**

In `litegen-core/Cargo.toml`, change line 23 from:

```toml
tower-http = { version = "0.6", features = ["cors", "trace", "compression-gzip", "timeout"] }
```

to:

```toml
tower-http = { version = "0.6", features = ["cors", "trace", "compression-gzip", "timeout", "fs"] }
```

- [ ] **Step 2: Write the failing test**

Open `litegen-core/src/api/mod.rs`. Add this test module at the end of the file (adjust the `use super::*;` if the module already has one — keep a single tests module):

```rust
#[cfg(test)]
mod dashboard_fallback_tests {
    use super::attach_dashboard;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    fn write_tmp_dashboard() -> std::path::PathBuf {
        // Unique temp dir without external crates: pid + monotonic-ish counter.
        let mut dir = std::env::temp_dir();
        dir.push(format!("litegen-dash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), b"<!doctype html><title>DASH</title>").unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/app.js"), b"console.log('app')").unwrap();
        dir
    }

    fn api_only() -> Router {
        Router::new().route("/v1/models", get(|| async { "MODELS" }))
    }

    #[tokio::test]
    async fn serves_index_at_root() {
        let dir = write_tmp_dashboard();
        let app = attach_dashboard(api_only(), &dir);
        let res = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("DASH"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn deep_link_falls_back_to_index() {
        let dir = write_tmp_dashboard();
        let app = attach_dashboard(api_only(), &dir);
        let res = app
            .oneshot(Request::get("/some/spa/route").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("DASH"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn api_route_not_shadowed_by_fallback() {
        let dir = write_tmp_dashboard();
        let app = attach_dashboard(api_only(), &dir);
        let res = app
            .oneshot(Request::get("/v1/models").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"MODELS");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn serves_static_asset() {
        let dir = write_tmp_dashboard();
        let app = attach_dashboard(api_only(), &dir);
        let res = app
            .oneshot(Request::get("/assets/app.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd litegen-core && cargo test --lib dashboard_fallback_tests 2>&1 | tail -20`
Expected: FAIL to **compile** — `attach_dashboard` not found, and possibly `http-body-util` / `tower` dev-deps missing.

- [ ] **Step 4: Ensure dev-dependencies exist for the test**

Check `litegen-core/Cargo.toml` for a `[dev-dependencies]` section. Ensure these are present (add any that are missing):

```toml
[dev-dependencies]
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
```

(`tokio` is already a full-feature dependency, so `#[tokio::test]` works. If `tower`/`http-body-util` are already dev-deps with different versions, keep the existing versions — just make sure the `util` feature is on for `tower`.)

- [ ] **Step 5: Implement `attach_dashboard`**

In `litegen-core/src/api/mod.rs`, add this public function (near the top of the module, after the existing `use`/`mod` lines):

```rust
use std::path::Path;

/// Attach a single-page-app fallback that serves a pre-built dashboard from `dir`.
///
/// Only requests that match no API route reach this fallback. Unknown paths
/// (client-side routes like `/login`) resolve to `index.html` so the SPA router
/// can take over; real files under `dir` (e.g. `/assets/app.js`) are served
/// directly. Call this AFTER the router's state has been set (`with_state`),
/// i.e. on the `Router<()>` returned by `create_router`.
pub fn attach_dashboard(router: axum::Router, dir: &Path) -> axum::Router {
    use tower_http::services::{ServeDir, ServeFile};

    let index = dir.join("index.html");
    let serve = ServeDir::new(dir).fallback(ServeFile::new(index));
    router.fallback_service(serve)
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd litegen-core && cargo test --lib dashboard_fallback_tests 2>&1 | tail -20`
Expected: PASS — `test result: ok. 4 passed`.

- [ ] **Step 7: Commit**

```bash
git add litegen-core/Cargo.toml litegen-core/src/api/mod.rs
git commit -m "feat(server): attach_dashboard SPA fallback (tower-http fs) + tests"
```

---

## Task 2: Rust — open-auth startup warning predicate

**Files:**
- Modify: `litegen-core/src/config/mod.rs` (add fn + test in existing `mod tests` at `:548`)

- [ ] **Step 1: Write the failing test**

In `litegen-core/src/config/mod.rs`, inside the existing `#[cfg(test)] mod tests { ... }` (starts at line 548), add:

```rust
#[test]
fn warn_open_auth_only_single_tenant_without_key() {
    use super::{warn_open_auth, Mode};
    assert!(warn_open_auth(Mode::SingleTenant, false));   // open: warn
    assert!(!warn_open_auth(Mode::SingleTenant, true));   // key set: quiet
    assert!(!warn_open_auth(Mode::Hosted, false));        // hosted: not this warning's concern
    assert!(!warn_open_auth(Mode::Hosted, true));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd litegen-core && cargo test --lib warn_open_auth_only_single_tenant_without_key 2>&1 | tail -15`
Expected: FAIL to compile — `warn_open_auth` not found.

- [ ] **Step 3: Implement the predicate**

In `litegen-core/src/config/mod.rs`, add this free function (place it near the `Mode` impl, e.g. after line 13):

```rust
/// True when the server is about to run with no API authentication: single-tenant
/// mode and no master key configured. Used to emit a one-time startup warning.
pub fn warn_open_auth(mode: Mode, has_master_key: bool) -> bool {
    matches!(mode, Mode::SingleTenant) && !has_master_key
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd litegen-core && cargo test --lib warn_open_auth_only_single_tenant_without_key 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/config/mod.rs
git commit -m "feat(config): warn_open_auth predicate for unauthenticated single-tenant boot"
```

---

## Task 3: Rust — wire fallback + warning into `main.rs`

**Files:**
- Modify: `litegen-core/src/main.rs` (`:55` area and `:232-234`)

- [ ] **Step 1: Emit the open-auth warning at startup**

In `litegen-core/src/main.rs`, immediately after the `info!(... "Starting LiteGen proxy")` block (ends line 55), add:

```rust
    if litegen::config::warn_open_auth(config.mode, config.master_key.is_some()) {
        tracing::warn!(
            "No LITEGEN__MASTER_KEY set — the API is UNAUTHENTICATED. Anyone who can reach \
             this port can use it. Set LITEGEN__MASTER_KEY before exposing it to a network."
        );
    }
```

- [ ] **Step 2: Attach the dashboard fallback conditionally**

In `litegen-core/src/main.rs`, replace the "Build axum router" block (lines 231-234):

```rust
    // Build axum router
    let app = api::create_router(state)
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());
```

with:

```rust
    // Build axum router. When a pre-built dashboard is bundled (Docker image sets
    // LITEGEN_DASHBOARD_DIR=/app/dashboard), serve it as a fallback for any path
    // the API doesn't claim. Absent/missing dir (dev, `cargo run`) → API only.
    let mut app = api::create_router(state);
    if let Ok(dir) = std::env::var("LITEGEN_DASHBOARD_DIR") {
        let path = std::path::PathBuf::from(&dir);
        if path.join("index.html").is_file() {
            info!(dashboard_dir = %dir, "Serving bundled dashboard UI");
            app = litegen::api::attach_dashboard(app, &path);
        } else {
            tracing::warn!(dashboard_dir = %dir, "LITEGEN_DASHBOARD_DIR set but no index.html found — not serving a dashboard");
        }
    }
    let app = app
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());
```

- [ ] **Step 3: Verify the crate builds and all tests pass**

Run: `cd litegen-core && cargo build --bin litegen 2>&1 | tail -15 && cargo test --lib 2>&1 | tail -15`
Expected: build succeeds; `test result: ok` for the lib tests (including Task 1 + Task 2 tests).

- [ ] **Step 4: Manual smoke — dashboard served from a temp dir**

```bash
cd litegen-core
mkdir -p /tmp/litegen-dash && printf '<!doctype html><title>SMOKE</title>' > /tmp/litegen-dash/index.html
LITEGEN_DASHBOARD_DIR=/tmp/litegen-dash LITEGEN__DATABASE_URL='sqlite:///tmp/litegen-smoke.db?mode=rwc' \
  cargo run --bin litegen >/tmp/litegen-smoke.log 2>&1 &
SRV=$!; sleep 8
echo "--- root should be SMOKE html ---"; curl -s http://127.0.0.1:4000/ | head -c 80; echo
echo "--- health still JSON/ok ---"; curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:4000/health/live
echo "--- open-auth warning present? ---"; grep -i 'UNAUTHENTICATED' /tmp/litegen-smoke.log && echo FOUND || echo MISSING
kill $SRV 2>/dev/null
```
Expected: root returns the `SMOKE` HTML; `/health/live` returns `200`; the log contains the `UNAUTHENTICATED` warning.

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/main.rs
git commit -m "feat(server): serve bundled dashboard when LITEGEN_DASHBOARD_DIR is set; warn on open auth"
```

---

## Task 4: Three-stage Dockerfile

**Files:**
- Rewrite: `Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Create `.dockerignore`**

Create `.dockerignore` at repo root:

```
**/node_modules
**/target
**/dist
**/.next
**/.next-prod
**/out
.git
.github
docs
apps
deploy
*.log
.env*
!.env.example
**/.DS_Store
```

(We intentionally do NOT ignore `litegen-core/migrations`, `models`, `dashboard`, or `sdks/typescript` — they are build inputs.)

- [ ] **Step 2: Rewrite `Dockerfile`**

Replace the entire `Dockerfile` with:

```dockerfile
# syntax=docker/dockerfile:1

# ── Stage 1: build the dashboard SPA (and its local SDK dependency) ──────────
FROM node:22-slim AS dashboard
WORKDIR /build
# The dashboard depends on @litegen/sdk via file:../sdks/typescript, which has
# no prepare script and a gitignored dist/ — build the SDK first.
COPY sdks/typescript/package.json sdks/typescript/package-lock.json* sdks/typescript/
RUN cd sdks/typescript && npm ci
COPY sdks/typescript/ sdks/typescript/
RUN cd sdks/typescript && npm run build
COPY dashboard/package.json dashboard/package-lock.json* dashboard/
RUN cd dashboard && npm ci
COPY dashboard/ dashboard/
# VITE_API_URL=/ → API base resolves to '' → the SPA calls /v1/... same-origin.
# (An EMPTY string would wrongly fall back to localhost; '/' is required.)
RUN cd dashboard && VITE_API_URL=/ npm run build

# ── Stage 2: build the Rust binary ───────────────────────────────────────────
# Rust 1.85+ required: transitive deps (e.g. time-macros) need edition 2024.
FROM rust:1.94-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates wget && rm -rf /var/lib/apt/lists/*
COPY litegen-core/Cargo.toml litegen-core/Cargo.lock litegen-core/
COPY litegen-core/src litegen-core/src
COPY litegen-core/migrations litegen-core/migrations
COPY models models
WORKDIR /build/litegen-core
RUN cargo build --release --bin litegen

# ── Stage 3: runtime ─────────────────────────────────────────────────────────
# Must match the builder's Debian release (trixie) so glibc versions line up.
FROM debian:trixie-slim
ARG VERSION=0.0.0
ARG REVISION=unknown
ARG BUILD_DATE=unknown
LABEL org.opencontainers.image.title="LiteGen" \
      org.opencontainers.image.description="Universal proxy for AI image and video generation — API + dashboard." \
      org.opencontainers.image.source="https://github.com/visgotti/litegen-first" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.created="${BUILD_DATE}"
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates sqlite3 wget tini && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/litegen-core/target/release/litegen /app/litegen
COPY models /app/models
COPY --from=dashboard /build/dashboard/dist /app/dashboard
# Persistent data (SQLite DB lives here). Owned by the unprivileged runtime uid.
RUN mkdir -p /data && chown -R 1000:1000 /data
VOLUME /data
ENV LITEGEN_VERSION=${VERSION} \
    LITEGEN_MODELS_DIR=/app/models \
    LITEGEN_DASHBOARD_DIR=/app/dashboard \
    LITEGEN__SERVER__HOST=0.0.0.0 \
    LITEGEN__SERVER__PORT=4000 \
    LITEGEN__DATABASE_URL=sqlite:///data/litegen.db?mode=rwc \
    RUST_LOG=info
EXPOSE 4000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD wget -qO- http://127.0.0.1:4000/health/live || exit 1
USER 1000:1000
ENTRYPOINT ["/usr/bin/tini", "--", "/app/litegen"]
```

- [ ] **Step 3: Build the image for the local arch**

Run: `docker build -t litegen:dev --build-arg VERSION=0.0.0-dev . 2>&1 | tail -25`
Expected: all three stages complete; final line shows the image built/tagged. (First build is slow — Rust release compile.)

- [ ] **Step 4: Smoke-test the SQLite one-liner**

```bash
docker rm -f litegen-smoke 2>/dev/null
docker run -d --name litegen-smoke -p 4000:4000 -v litegen-smoke-data:/data litegen:dev
sleep 12
echo "--- health ---"; curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:4000/health/live
echo "--- dashboard root is HTML ---"; curl -s http://127.0.0.1:4000/ | grep -iq '<!doctype html' && echo OK || echo FAIL
echo "--- API route reachable (not shadowed) ---"; curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:4000/v1/models
echo "--- SQLite file created in volume ---"; docker exec litegen-smoke sh -c 'ls -la /data/litegen.db' || echo "no db"
echo "--- container HEALTHCHECK status ---"; docker inspect -f '{{.State.Health.Status}}' litegen-smoke
echo "--- open-auth warning in logs ---"; docker logs litegen-smoke 2>&1 | grep -i UNAUTHENTICATED && echo FOUND || echo MISSING
docker rm -f litegen-smoke; docker volume rm litegen-smoke-data
```
Expected: health `200`; root is HTML; `/v1/models` returns `200`; `/data/litegen.db` exists; health status becomes `healthy`; the UNAUTHENTICATED warning is present.

- [ ] **Step 5: Verify same-origin (no localhost leaked into the bundle)**

Run: `docker run --rm litegen:dev sh -c 'grep -rl "localhost:4000" /app/dashboard || echo NONE'`
Expected: `NONE` (the dashboard bundle must not contain the localhost fallback string — confirms `VITE_API_URL=/` worked).

- [ ] **Step 6: Commit**

```bash
git add Dockerfile .dockerignore
git commit -m "feat(docker): three-stage image bundling API + dashboard (OCI labels, tini, healthcheck, SQLite /data)"
```

---

## Task 5: Compose files + env template

**Files:**
- Rewrite: `docker-compose.yml`
- Create: `docker-compose.build.yml`
- Create: `.env.example`

- [ ] **Step 1: Move the build-from-source compose**

Create `docker-compose.build.yml` with the CURRENT contents of `docker-compose.yml` (build from source, for contributors). Its `litegen` service keeps `build: .` and `image: litegen:latest`. (Copy the existing file verbatim into the new name.)

- [ ] **Step 2: Rewrite `docker-compose.yml` to pull the published image**

Replace `docker-compose.yml` with:

```yaml
# Production compose: pulls the published LiteGen image and runs it with Postgres.
# Quickest path (no Postgres) is the one-liner in the README; this is for scale.
#   1) cp .env.example .env  &&  edit it
#   2) docker compose up -d
services:
  litegen:
    image: ${LITEGEN_IMAGE:-litegen/litegen:1}
    ports:
      - "4000:4000"
    environment:
      LITEGEN__DATABASE_URL: "postgres://litegen:${POSTGRES_PASSWORD:-litegen}@db:5432/litegen"
      LITEGEN__MASTER_KEY: "${LITEGEN__MASTER_KEY:-}"
      LITEGEN_CORS_ORIGINS: "${LITEGEN_CORS_ORIGINS:-}"
      OPENAI_API_KEY: "${OPENAI_API_KEY:-}"
      STABILITY_API_KEY: "${STABILITY_API_KEY:-}"
      REPLICATE_API_TOKEN: "${REPLICATE_API_TOKEN:-}"
      GOOGLE_API_KEY: "${GOOGLE_API_KEY:-}"
      FAL_KEY: "${FAL_KEY:-}"
      RUST_LOG: "info"
    depends_on:
      db:
        condition: service_healthy
    restart: unless-stopped
    # The image ships its own HEALTHCHECK.

  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: litegen
      POSTGRES_PASSWORD: "${POSTGRES_PASSWORD:-litegen}"
      POSTGRES_DB: litegen
    volumes:
      - litegen_db:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U litegen"]
      interval: 5s
      timeout: 3s
      retries: 5
    restart: unless-stopped

volumes:
  litegen_db:
```

- [ ] **Step 3: Create `.env.example`**

```bash
# Copy to .env and edit. Used by `docker compose up -d` (production Postgres path).

# Pin the LTS image. ':1' = latest stable v1.x (auto-patch). Or pin ':1.2.3'.
LITEGEN_IMAGE=litegen/litegen:1

# Postgres password (the compose db service uses the same value).
POSTGRES_PASSWORD=change-me

# Set a master key to require auth on the API (STRONGLY recommended for any
# network-exposed deployment). Leave blank only for trusted local use.
LITEGEN__MASTER_KEY=

# Comma-separated allowed CORS origins for browser clients (optional).
LITEGEN_CORS_ORIGINS=

# Provider keys (optional — add the ones you use).
OPENAI_API_KEY=
STABILITY_API_KEY=
REPLICATE_API_TOKEN=
GOOGLE_API_KEY=
FAL_KEY=
```

- [ ] **Step 4: Validate both compose files parse**

Run:
```bash
docker compose -f docker-compose.yml config >/dev/null && echo "prod compose OK"
docker compose -f docker-compose.build.yml config >/dev/null && echo "build compose OK"
```
Expected: both print `OK` (no YAML/interpolation errors). A warning that `.env` is absent is fine.

- [ ] **Step 5: Commit**

```bash
git add docker-compose.yml docker-compose.build.yml .env.example
git commit -m "feat(docker): pull-based prod compose + build-from-source compose + .env.example"
```

---

## Task 6: GitHub Actions multi-arch publish

**Files:**
- Create: `.github/workflows/docker-publish.yml`

- [ ] **Step 1: Create the workflow**

Create `.github/workflows/docker-publish.yml`:

```yaml
name: Publish Docker image

on:
  push:
    tags: ["v*.*.*"]
  workflow_dispatch:
    inputs:
      tag:
        description: "Version to build (e.g. v1.2.3). Defaults to latest git tag."
        required: false

env:
  # Flip these if you don't own the `litegen` namespace — e.g. visgotti/litegen.
  DOCKERHUB_IMAGE: litegen/litegen
  GHCR_IMAGE: ghcr.io/${{ github.repository_owner }}/litegen

jobs:
  publish:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write
    steps:
      - uses: actions/checkout@v4

      - name: Derive version
        id: ver
        run: |
          REF="${{ github.event.inputs.tag || github.ref_name }}"
          echo "version=${REF#v}" >> "$GITHUB_OUTPUT"
          echo "created=$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$GITHUB_OUTPUT"

      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3

      - name: Log in to Docker Hub
        uses: docker/login-action@v3
        with:
          username: ${{ secrets.DOCKERHUB_USERNAME }}
          password: ${{ secrets.DOCKERHUB_TOKEN }}

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Compute tags + labels
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: |
            ${{ env.DOCKERHUB_IMAGE }}
            ${{ env.GHCR_IMAGE }}
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=semver,pattern={{major}}
            type=raw,value=latest

      - name: Build and push (amd64 + arm64)
        uses: docker/build-push-action@v6
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          build-args: |
            VERSION=${{ steps.ver.outputs.version }}
            REVISION=${{ github.sha }}
            BUILD_DATE=${{ steps.ver.outputs.created }}
          sbom: true
          provenance: true
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

- [ ] **Step 2: Lint the workflow YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/docker-publish.yml')); print('workflow YAML OK')"`
Expected: `workflow YAML OK`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/docker-publish.yml
git commit -m "ci: tag-driven multi-arch Docker publish to Docker Hub + GHCR (SBOM + provenance)"
```

- [ ] **Step 4: Document the manual publish prerequisites (no code)**

Note for the user (do not commit as code — this is handoff info):
- Add repo secrets `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` (a Docker Hub access token).
- Confirm the `litegen` Docker Hub namespace is yours; if not, edit `DOCKERHUB_IMAGE` in the workflow (and the compose/docs) to `visgotti/litegen`.
- Publish a release by pushing a tag: `git tag v0.1.0 && git push origin v0.1.0`.

---

## Task 7: Docs — README quick start + Docker Hub description

**Files:**
- Create: `docker/README.md`
- Modify: `README.md` (the `## Quick Start` section, ~lines 70-116, and the env-var reference table near line 324)

- [ ] **Step 1: Create `docker/README.md` (Docker Hub description)**

```markdown
# LiteGen

The universal proxy for AI image and video generation — like LiteLLM, but for
multimedia. One image, one port: the OpenAI-compatible API **and** the dashboard UI.

## Run it (SQLite, zero dependencies)

```bash
docker run -d --name litegen -p 4000:4000 -v litegen:/data \
  -e OPENAI_API_KEY=sk-... \
  litegen/litegen:1
```

- API: `http://localhost:4000/v1/...`
- Dashboard: `http://localhost:4000`
- Health: `http://localhost:4000/health/live`

Data (the SQLite database) persists in the `litegen` volume.

> By default the API runs **unauthenticated**. Set `-e LITEGEN__MASTER_KEY=<secret>`
> before exposing it to a network.

## Production (Postgres)

Use the `docker-compose.yml` from the repo, which pulls this image alongside Postgres.

## Tags

- `1.2.3` — exact, immutable.
- `1.2` / `1` — floating: latest patch within that minor/major (the LTS pin).
- `latest` — most recent release.

## Common configuration

| Env var | Default | Purpose |
|---|---|---|
| `LITEGEN__MASTER_KEY` | (unset) | Bearer token required on the API when set. |
| `LITEGEN__DATABASE_URL` | `sqlite:///data/litegen.db?mode=rwc` | SQLite path or `postgres://...`. |
| `LITEGEN_CORS_ORIGINS` | (none) | Comma-separated browser origins. |
| `OPENAI_API_KEY`, `REPLICATE_API_TOKEN`, `GOOGLE_API_KEY`, `FAL_KEY`, ... | (unset) | Provider credentials. |

Full reference: the project README.
```

- [ ] **Step 2: Rewrite the README `## Quick Start` section**

In `README.md`, replace the `### Docker (recommended)` subsection (the `git clone ... docker compose up -d` block) so the pull-based one-liner is first:

```markdown
### Docker (recommended)

Run the whole thing — API **and** dashboard — from one image, no clone, no build:

```bash
docker run -d --name litegen -p 4000:4000 -v litegen:/data \
  -e OPENAI_API_KEY=sk-... \
  litegen/litegen:1
```

- API at `http://localhost:4000/v1/...`, dashboard at `http://localhost:4000`,
  health at `http://localhost:4000/health/live`.
- The SQLite database persists in the `litegen` volume.
- **Auth is open by default.** Add `-e LITEGEN__MASTER_KEY=<secret>` before
  exposing it to any network.

**Production (Postgres):**

```bash
git clone https://github.com/visgotti/litegen-first.git litegen && cd litegen
cp .env.example .env   # edit: set POSTGRES_PASSWORD + LITEGEN__MASTER_KEY
docker compose up -d
```

**Build from source** (contributors): `docker compose -f docker-compose.build.yml up -d --build`.

#### Image tags

- `litegen/litegen:1.2.3` — exact, immutable.
- `litegen/litegen:1` (or `:1.2`) — latest patch within that major/minor; pin this for automatic, compatible updates (the LTS contract).
- `litegen/litegen:latest` — most recent release.
- Also on GHCR: `ghcr.io/visgotti/litegen`. Images are multi-arch (amd64 + arm64).
```

- [ ] **Step 3: Add/extend the env-var reference + persistence note**

In `README.md`, near the existing env-var table (~line 324), ensure these rows exist (add any missing):

```markdown
| `LITEGEN__MASTER_KEY` | No | (unset) | When set, the API requires `Authorization: Bearer <key>`. Unset = open. |
| `LITEGEN__SERVER__HOST` / `LITEGEN__SERVER__PORT` | No | `0.0.0.0` / `4000` | Bind address/port. |
| `LITEGEN_DASHBOARD_DIR` | No | `/app/dashboard` (in image) | Directory of the bundled dashboard SPA; unset/missing ⇒ API-only. |
| `LITEGEN_VERSION` | No | (baked) | Image version string, set at build time. |
```

And add a short persistence subsection:

```markdown
#### Persistence & upgrades

- Mount a volume at `/data` — the SQLite database lives there (`/data/litegen.db`).
  With the default `local` image-storage backend, generated images are not written
  to disk; configure the S3 backend (`LITEGEN__IMAGE_STORAGE__BACKEND=s3` + the
  `LITEGEN_S3_*` vars) to persist artifacts at scale.
- Upgrade by pulling a newer tag and recreating the container. Pin `:1` to get
  compatible patches automatically, or `:1.2.3` for fully reproducible deploys.
```

- [ ] **Step 4: Verify README renders / links are intact**

Run: `grep -nE 'litegen/litegen:1|docker run -d --name litegen|docker-compose.build.yml' README.md`
Expected: matches showing the one-liner, the `:1` tag, and the build-compose reference are present.

- [ ] **Step 5: Commit**

```bash
git add README.md docker/README.md
git commit -m "docs: pull-based Docker quick start (one image: API + dashboard), tags, persistence, auth"
```

---

## Final verification (after all tasks)

- [ ] **Full Rust test suite:** `cd litegen-core && cargo test --lib 2>&1 | tail -15` → all pass.
- [ ] **Local image build + one-liner smoke** (Task 4 Steps 3-5) pass.
- [ ] **Multi-arch build sanity** (no push):
  `docker buildx build --platform linux/amd64,linux/arm64 --build-arg VERSION=0.0.0-test -t litegen:multiarch-test . 2>&1 | tail -15`
  Expected: both platforms build successfully.
- [ ] **Compose parses:** `docker compose config >/dev/null && echo OK`.
- [ ] **Workflow YAML valid** (Task 6 Step 2).
- [ ] **Open items for the user:** confirm Docker Hub namespace ownership; add `DOCKERHUB_USERNAME`/`DOCKERHUB_TOKEN` secrets; push a `vX.Y.Z` tag to publish.

---

## Spec coverage check

| Spec section | Task(s) |
|---|---|
| §4.1 three-stage Dockerfile | Task 4 |
| §4.2 Rust dashboard fallback | Tasks 1, 3 |
| §4.3 open-auth warning | Tasks 2, 3 |
| §4.4 quickstart one-liner + Postgres compose | Tasks 4, 5, 7 |
| §4.5 versioning + CI | Task 6 |
| §4.6 docs | Task 7 |
| §7 image-name caveat | Task 6 (env var), Task 7 (docs), Final verification |
