# LiteGen hosted deploy — design

**Date:** 2026-05-30
**Status:** approved, implementing
**Goal:** A `deploy.js` that stands up LiteGen's own hosted offering (à la Langfuse Cloud): the
proxy + Postgres on a DigitalOcean droplet, and the marketing landing site on Cloudflare Pages —
driven by a gitignored `.env.deploy` with a committed `.env.deploy.template`.

## Context

- **Proxy** = `litegen-core` (Rust binary), already containerized via root [`Dockerfile`](../../../Dockerfile) +
  [`docker-compose.yml`](../../../docker-compose.yml).
- **DB** = Postgres (already a service in the compose file).
- **Landing** = `apps/landing` — Next.js 15 App Router + `next-intl`. Already static-export-ready
  (`generateStaticParams()` + `setRequestLocale()` present, locales `['en','es']`, no `next/image`);
  the only blocker to `output: 'export'` is `src/middleware.ts`.
- **Reference**: `../storefront/deploy.js` — mirror its conventions, but it targets a *pre-existing*
  droplet (`DO_DROPLET_IP` set by hand). LiteGen adds a `provision` step that **creates** the droplet
  via the DO API.
- **Creds** come from `../storefront/.env.deploy`: `DO_API_KEY`, `CLOUDFLARE_API_TOKEN`,
  `CLOUDFLARE_ACCOUNT_ID`, `GHCR_TOKEN`.
- **Toolchain**: node v25 (global `fetch`, no DO SDK needed), Docker 29 + buildx, SSH key `~/.ssh/id_ed25519`.

## Decisions (locked with user)

1. **Cloudflare**: static export → CF Pages (rework `next-intl` to fully static, drop middleware).
2. **Proxy delivery**: build image locally → push GHCR → droplet pulls.
3. **Scope**: build *and* run live this session (real droplet, real CF deploy).
4. Plain `http://<ip>:4000` for the proxy for now (no domain/TLS yet) — Cloudflare-fronted TLS later.

## Components

### `deploy.js` (root)
Single orchestrator. Reuses storefront's `.env.deploy` loader and `env()/required()/run()/runCapture()/sshExec()`
helpers and the `assertNoLocalhostInBundle()` guard. Targets:

- `provision` — ensure DO droplet exists; persist `DO_DROPLET_IP` to `.env.deploy`.
- `proxy` — buildx amd64 image → push GHCR → SSH: write compose + `.env`, `docker compose pull && up -d`, health-check.
- `landing` — static build → guard → `wrangler pages deploy`.
- `all` — `provision` → `proxy` → `landing`.
- `--dry-run` — log intended actions, make no real API/SSH calls.

### Provisioning (DO API, `fetch`)
1. `GET /v2/droplets?tag_name=litegen` — reuse existing `litegen-proxy` droplet if present (idempotent; avoids duplicate billing).
2. Ensure account SSH key (`id_ed25519.pub`) via `GET/POST /v2/account/keys`.
3. `POST /v2/droplets`: size `s-1vcpu-2gb` (~$12/mo), region `nyc3`, image `ubuntu-24-04-x64`, tag `litegen`,
   `monitoring: true`. Size/region/image overridable via `DO_SIZE`/`DO_REGION`/`DO_IMAGE`.
4. Poll to `active` + public IPv4; wait for SSH:22; write `DO_DROPLET_IP` back to `.env.deploy`.

### Proxy + DB on droplet
- `docker buildx build --platform linux/amd64 --push -t ghcr.io/visgotti/litegen:<sha> -t :latest .` (Mac arm64 → droplet amd64).
- SSH: install Docker via `get.docker.com` (idempotent); `docker login ghcr.io`; write
  `deploy/docker-compose.prod.yml` (litegen `${LITEGEN_IMAGE}` + `postgres:16-alpine` + named volume) and a `.env`
  (generated `LITEGEN__MASTER_KEY` + Postgres password, both persisted back to `.env.deploy`); `docker compose pull && up -d`.
- Expose `http://<ip>:4000`; health-check `GET /health/live`, dump `docker compose logs` on failure; prune old images.

### Landing on Cloudflare Pages
- Delete `src/middleware.ts`; `next.config.ts` gets `output: 'export'` + `images: { unoptimized: true }`;
  add `public/_redirects` → `/  /en  302`.
- Build → `apps/landing/out`; `assertNoLocalhostInBundle('apps/landing/out')`;
  `wrangler pages deploy apps/landing/out --project-name=litegen-landing --branch=main` (auto-creates project →
  `litegen-landing.pages.dev`).

## Files

| File | Status |
|---|---|
| `deploy.js` | new |
| `.env.deploy.template` | new, committed |
| `.env.deploy` | new, gitignored |
| `deploy/docker-compose.prod.yml` | new |
| `.gitignore` | edit: add `!.env.deploy.template` |
| `package.json` (root) | edit: `node-ssh` + `wrangler` devDeps, `deploy*` scripts |
| `apps/landing/next.config.ts` | edit: `output: 'export'` |
| `apps/landing/src/middleware.ts` | delete |
| `apps/landing/public/_redirects` | new |

## Error handling & safety

- `required()` aborts on any missing cred before doing work.
- Provision is idempotent (reuse by tag + name); generated secrets persist so re-runs don't rotate the master key.
- Cross-platform image build pinned to `linux/amd64`.
- Health checks gate "success"; logs dumped on failure.
- `.env.deploy` stays out of git; only `.env.deploy.template` (placeholders) is committed.
- Credential copy from storefront happens file→file, never echoed to logs/chat.

## Out of scope (for now)

- Custom domain + TLS termination (revisit once a domain exists; front proxy with Cloudflare).
- CI-driven image builds (GitHub Actions) — local buildx for now.
- Multi-droplet / horizontal scaling.
