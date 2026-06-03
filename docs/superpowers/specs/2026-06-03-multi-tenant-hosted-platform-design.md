---
title: Multi-tenant hosted platform — orgs, apps, BYO provider keys, id/secret API keys
status: approved
owner: joe
date: 2026-06-03
---

# Goal

Turn LiteGen from a **single-tenant self-hosted proxy** into a **multi-tenant hosted product** (à la
Langfuse Cloud): we host the dashboard + Rust proxy; customers self-serve sign up, create an
**Organization** (with teammates), spin up **Applications**, and get **id/secret API key pairs** they
drop into their own apps pointed at our single URL — working **out of the box** with **their own
(bring-your-own) upstream provider keys**.

This evolves the prior [users/roles/permissions spec](2026-05-29-users-roles-permissions-design.md)
(which listed "multi-tenant orgs/teams" as an explicit non-goal) and the prior
[hosted-deploy spec](2026-05-30-litegen-hosted-deploy-design.md) (single droplet, single master key).

## Decisions (locked with user)

1. **Billing / provider keys: bring-your-own (BYO).** Each app stores its own encrypted upstream
   provider credentials (OpenAI/Fal/Replicate/…). We proxy, route, meter, and observe; we never front
   generation cost. (Global env-configured provider keys remain a fallback for single-tenant mode.)
2. **Tenancy: Org (with teams) → Apps → keys.** Orgs can invite teammates with per-org roles
   (owner/admin/member/viewer) from day one. Full Langfuse-parity hierarchy.
3. **API keys: id + secret, secret used as Bearer.** Each key = public id `pk_live_…` (display/reference)
   + secret `sk_live_…` (hashed at rest, shown once). Apps send `Authorization: Bearer sk_live_…` —
   drop-in for OpenAI-style SDKs pointed at our URL. The secret resolves the tenant.
4. **Mode switch** `LITEGEN__MODE = single_tenant | hosted` (default `single_tenant`) preserves existing
   self-host behavior and keeps current tests green. Every query is **always** tenant-scoped; mode only
   changes signup policy and master-key reach.
5. **Phase 1 first** (multi-tenant identity & auth core + full test suites). Real infrastructure
   provisioning is **gated on explicit $ approval** and comes after Phases 1–2 are green.

# Non-goals (this spec)

- Real infra provisioning (managed Postgres/Redis, LB, TLS, DNS) — that's Phase 4, separately approved.
- Horizontal multi-instance correctness (Redis-backed rate limit/circuit breaker, poller leasing,
  object storage) — Phase 3, separate sub-spec.
- Stripe/usage billing UI. We meter usage per org/app; invoicing is a follow-up.
- Per-app custom roles / fine-grained permission editing. Roles remain the 4 predefined ones, now per-org.
- SSO/SAML, 2FA. (OAuth GitHub+Google continues to work, now org-aware.)

# Architecture overview

```
User (global login identity, email-unique)
  └─ organization_members (role: owner/admin/member/viewer)
       └─ Organization (tenant: slug, plan, status)
            ├─ members (invite teammates; per-org roles)
            ├─ Applications (project unit: "prod", "staging", …)
            │    ├─ api_keys      → public_id pk_live_… + secret sk_live_… (secret_hash, shown once)
            │    └─ provider_credentials (BYO, AES-256-GCM encrypted: openai/fal/replicate/…)
            └─ all data (generations, request_logs, request_artifacts, audit_log, webhook_deliveries)
               carries org_id + app_id and is filtered by it on every read/write
```

Request authentication resolves a **tenant context** two ways:

- **Programmatic** (customer apps): `Authorization: Bearer sk_live_…` → `sha256` → key row →
  `{org_id, app_id, scopes, quota}`. No extra header — works out of the box.
- **Dashboard** (humans): `litegen_session` cookie → user; the **active org+app** is sent by the SDK as
  `X-Litegen-Org-Id` / `X-Litegen-App-Id` headers and **validated against membership server-side** on
  every request (absent → user's default org/app). New org/app/member CRUD uses explicit paths.

# Data model

All migrations are numbered after the existing series (`…0007_users.sql`) and **mirrored in
`migrations/postgres` + `migrations/sqlite`**. A single migration backfills existing rows into a
default org/app so current single-tenant deployments keep working post-migration.

## New tables

### `organizations`  (`…0008_organizations.sql`)
| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PK | UUID v4 |
| `name` | TEXT NOT NULL | display name |
| `slug` | TEXT NOT NULL UNIQUE | url-safe, derived from name + dedup suffix |
| `plan` | TEXT NOT NULL DEFAULT 'free' | free/pro/… (informational for now) |
| `status` | TEXT NOT NULL DEFAULT 'active' | active/suspended |
| `created_at` / `updated_at` | TIMESTAMP | |

### `organization_members`
| Column | Type | Notes |
|---|---|---|
| `org_id` | TEXT NOT NULL REFERENCES organizations(id) | |
| `user_id` | TEXT NOT NULL REFERENCES users(id) | |
| `role` | TEXT NOT NULL | owner/admin/member/viewer (CHECK) |
| `created_at` | TIMESTAMP | |
| PRIMARY KEY | (`org_id`,`user_id`) | a user can belong to many orgs with different roles |

Index `(user_id)` for "list my orgs". Exactly one `owner` per org enforced at the app layer
(transfer-owner demotes the prior owner within that org only).

### `applications`
| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PK | UUID v4 |
| `org_id` | TEXT NOT NULL REFERENCES organizations(id) | |
| `name` | TEXT NOT NULL | |
| `slug` | TEXT NOT NULL | unique within org: UNIQUE(org_id, slug) |
| `status` | TEXT NOT NULL DEFAULT 'active' | |
| `created_at` / `updated_at` | TIMESTAMP | |

### `provider_credentials`  (schema in Phase 1, used in Phase 2)
| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PK | UUID v4 |
| `app_id` | TEXT NOT NULL REFERENCES applications(id) | per-app BYO creds |
| `provider` | TEXT NOT NULL | 'openai','fal','replicate',… |
| `ciphertext` | TEXT NOT NULL | base64 AES-256-GCM of a JSON `{api_key,key_id,key_secret,region,extra}` |
| `nonce` | TEXT NOT NULL | base64 12-byte GCM nonce |
| `display_hint` | TEXT NULL | e.g. `sk-…last4` for the UI; never the secret |
| `created_at` / `updated_at` | TIMESTAMP | |
| UNIQUE | (`app_id`,`provider`) | one credential per provider per app |

Encryption key from `LITEGEN__SECRETS_KEY` (base64, 32 bytes) via the `aes-gcm` crate. Plaintext is
**never** returned by any endpoint.

## Altered tables (add tenant columns + indexes)

- `api_keys`: add `org_id`, `app_id` (NOT NULL after backfill, FK), `public_id TEXT UNIQUE`
  (`pk_live_…`). Keep `key_hash` as the **secret hash** (`sha256(sk_live_…)`); old `lg-…` keys keep
  authenticating unchanged. `key_prefix` becomes the secret display prefix. `owner_user_id` stays
  (the minting user). New index `(org_id, app_id, created_at)`.
- `generations`, `request_logs`, `request_artifacts`, `webhook_deliveries`: add `org_id`, `app_id`
  (+ composite index leading with `org_id, app_id`). `request_logs`/`request_artifacts` had **no**
  owner column before — this is the isolation fix.
- `audit_log`: add `org_id` (keep `actor_user_id`, `actor_key_id`).
- `invitations`: add `org_id` (an invite joins a specific org with a role).
- `users`: unchanged identity (email globally unique = one login). `users.role` is repurposed as a
  **platform** role (`user` default, `platform_admin` for operators); tenant authz moves to
  `organization_members.role`.

## Migration backfill (`…0008`)
1. Create one `organizations` row (`slug='default'`, name from `LITEGEN__DEFAULT_ORG_NAME` or
   "Default"). Create one `applications` row (`slug='default'`) in it.
2. Set `org_id`/`app_id = default` on every existing `api_keys`, `generations`, `request_logs`,
   `request_artifacts`, `webhook_deliveries`, `audit_log`, `invitations` row.
3. For every existing `users` row, insert an `organization_members(default_org, user.id, user.role)`
   mapping their old global role into the default org.

# Auth, roles, and request flow

## Mode-conditioned signup / master key
- **`single_tenant`** (default): signup keeps the `409 signup_closed` once a user exists; the first
  signup joins the **default org** as owner (not a new org). `LITEGEN__MASTER_KEY` → full access scoped
  to the default org. Dev "accept any token" path → default-org admin. **Existing tests stay green.**
- **`hosted`**: `POST /v1/auth/signup` is open and creates **User + new Organization + owner membership
  + first Application** atomically. The master key is a **platform admin** (`org_id = None`): it may hit
  `/v1/admin/*` only; tenant-scoped routes with no resolvable org → `403`. The "accept any token" dev
  bypass is **disabled** (auth required).

## Tenant context middleware
`KeyContext` grows `org_id: Option<String>` and `app_id: Option<String>`. The auth middleware:
1. Bearer secret → key row → set `org_id`/`app_id` from the key (+ existing scopes/quota/rpm).
2. Else session cookie → user; read `X-Litegen-Org-Id`/`X-Litegen-App-Id`; **verify membership**
   (`organization_members`) → set `org_id` + role-derived permissions for that org; verify the app
   belongs to the org. Missing headers → user's default (most-recent) org + its first app.
3. Tenant-scoped handlers require `org_id` to be present and filter **every** query by it.

## Roles → permissions (per-org)
Reuse the `subject:verb[:scope]` scheme and `permissions_for(role)`, now evaluated against the
**membership role for the active org**. New permissions:
```
org:read        org:write        org:delete        org:transfer_owner
app:read        app:write        app:delete
member:read     member:invite    member:write      member:remove
provider_cred:read   provider_cred:write   provider_cred:delete
```
Existing key/generation/log/audit permissions keep their meaning but are now org-scoped. Bearer scope
auth (`generate`/`read`/`admin`) is unchanged and independent.

# API endpoints (Phase 1)

New:
- `POST /v1/orgs` — create org (also implicit on hosted signup). `GET /v1/orgs` — orgs I'm a member of.
- `GET/PATCH/DELETE /v1/orgs/{id}` — `org:read` / `org:write` / `org:delete`.
- `GET/POST /v1/orgs/{id}/members`, `PATCH/DELETE /v1/orgs/{id}/members/{user_id}`,
  `POST /v1/orgs/{id}/transfer-owner`.
- `GET/POST /v1/orgs/{id}/apps`, `GET/PATCH/DELETE /v1/apps/{app_id}`.
- `GET/POST /v1/apps/{app_id}/provider-credentials`, `DELETE …/{provider}` (write/delete in Phase 2;
  schema + endpoints stubbed in Phase 1).

Changed:
- `POST /v1/auth/signup` — body `{email, password, org_name?}`. Hosted: creates user+org+membership+app,
  returns session. Single-tenant: unchanged gate.
- `POST /v1/keys` — creates within the **active app**; response includes `public_id` + `secret`
  (`sk_live_…`, shown once) + `prefix`. `GET /v1/keys` — lists active app's keys (public_id + prefix,
  never secret). `rotate`/revoke scoped to the active org/app.
- `GET /v1/generations`, `/v1/logs`, `/v1/audit`, `/v1/stats` — filtered by `org_id` (+ `app_id`).
- `POST /v1/images|videos/generations` — Bearer secret resolves org/app; generation rows stamped with
  both. (BYO provider credential resolution lands in Phase 2; Phase 1 uses the mock provider + global
  fallback.)
- `GET /v1/auth/me` — returns `{user, orgs:[{id,name,role}], active_org, active_app}`.

All new endpoints get `#[utoipa::path]` annotations; SDK is regenerated (`sdks/scripts/regen-all.sh`)
so the dashboard and the god test dogfood them.

# BYO provider plumbing (Phase 2)
Per request: look up `provider_credentials(app_id, provider)` → AES-GCM decrypt → build
`ProviderCredentials` → thread `Option<ProviderCredentials>` through `router.generate(...) →
provider.generate(...)` (reusing the existing `ProviderCredentials::apply()` hook; ~28 provider impls
gain one optional param). If the app has not configured that provider → `400 provider_not_configured`.
Global env-configured keys remain a fallback (single-tenant + optional platform-offered providers).

# Dashboard (Phase 1)
- **Org/App context provider** above the router; **switcher** in `UserMenu` (org dropdown → app dropdown,
  persisted per-origin, namespaced by org). SDK injects `X-Litegen-Org-Id`/`X-Litegen-App-Id`.
- **Onboarding**: `/signup` gains an org-name field; post-signup "create your first app" step.
- **Scoped pages**: Overview / Logs / Generations / Keys read the active app. New **Members** + **Org
  settings** pages (org-level). Keys page shows `public_id` + secret-once banner (already present).
- API base URL: a single hosted origin (runtime, not the build-time `VITE_API_URL` localhost fallback).
- Permission-guided rendering uses the **active org's** membership role, not a global role.

# Testing strategy (hard requirement)

## Rust integration REST tests — `litegen-core/tests/multitenant_api.rs`
**Real server, real HTTP, fresh DB per test.** A `spawn_app()` helper binds an ephemeral `TcpListener`,
boots the axum app in `LITEGEN__MODE=hosted` against a fresh `tempfile` SQLite DB (migrations applied),
and returns `{ base_url, client: reqwest::Client }`. Each `#[tokio::test]` gets its own DB + server, so
there is zero shared state. (Mirrors the existing `axum-test`/`tempfile`/`wiremock` dev-deps; uses
`reqwest` against the bound port to "literally hit true endpoints".) A `wiremock` server stands in for
upstream providers via the `mock` provider model.

Test matrix (each an isolated `#[tokio::test]`):
1. **signup→org**: hosted signup creates user+org+owner membership+first app; sets session cookie;
   `GET /v1/auth/me` reflects it; `GET /v1/orgs` lists exactly one org.
2. **two tenants**: a second signup (different email) creates a **separate** org B.
3. **session lifecycle**: login, `/v1/auth/csrf`, mutating request without `X-CSRF-Token` → 403; with it
   → 200; logout deletes the session.
4. **app + key**: create app; create key → response has `public_id` (`pk_live_…`) + `secret`
   (`sk_live_…`); listing shows public_id + prefix and **never** the secret; the secret is returned only
   at creation.
5. **programmatic generate**: `Authorization: Bearer sk_live_…` → `POST /v1/images/generations`
   (`mock/...` model) → 200; the `generations`/`request_logs` rows carry the key's `org_id` + `app_id`.
6. **cross-tenant isolation** (the crux): org B's session (or key) cannot read org A's keys / generations
   / logs / members → `403`/`404`; org B session sending `X-Litegen-Org-Id:<orgA>` (not a member) → 403.
7. **invitations + roles**: org-A owner invites `member@…` as Member (token read via
   `LITEGEN__DEV__EXPOSE_INVITE_TOKENS`); accept → login → sees org A; Member/Viewer mutating attempts →
   `403 forbidden_permission`; Member cannot invite or read members; only Owner can `transfer-owner` /
   delete the org.
8. **key rotate/revoke**: revoked/rotated-away secret → `401` on generation.
9. **quota**: key with exhausted `token_quota` → `402` on generation.
10. **master-key/dev hardening (hosted)**: no auth → 401; master key → can hit `/v1/admin/*` but
    **cannot** read org A's tenant data (403); the "any token" dev bypass is off.
11. **provider creds (Phase 2)**: set encrypted cred for `openai` on the app → generation routes with it
    (wiremock asserts the upstream Authorization header); plaintext never returned; missing cred →
    `400 provider_not_configured`.

Acceptance: `cargo test --test multitenant_api` green; existing `cargo test` (unit/lib, single-tenant
god paths) stays green.

## Playwright e2e — `dashboard/e2e/multitenant.spec.ts` (+ extend `god-test.spec.ts`)
**Serves the real platform and recreates the DB each run.** `playwright.config.ts` `webServer` launches
a helper (`scripts/e2e-server.mjs`) that (a) **deletes the temp SQLite file and re-runs migrations**,
(b) boots `litegen-core` in `LITEGEN__MODE=hosted` with `LITEGEN__COOKIE_INSECURE_DEV=true` +
`LITEGEN__DEV__EXPOSE_INVITE_TOKENS=true` + the mock provider, and (c) serves the built dashboard
(`vite preview`) pointed at it. DB is recreated on suite start (fresh server); tests use unique
org/email slugs to stay independent. (Optional `POST /v1/admin/test-reset`, dev-flag-gated, for
per-test reset if needed.)

Flow driven entirely through the UI on `@litegen/sdk`:
1. Sign up org A owner → land in the new org with its first app.
2. Create a second app; create an API key → secret shown **once** → capture it.
3. (Phase 2) Add a provider credential (mock) in app settings.
4. Invite a teammate (Member); in a fresh browser context, accept via the dev-exposed token, set a
   password, sign in as Member → assert role-gated UI (no Org settings / Members management).
5. Use the org/app **switcher**.
6. **Isolation**: sign up org B in another context; assert neither org sees the other's apps/keys/logs.
7. Use the minted key (a `request.post` from the test, or the Playground UI) to generate, then confirm
   the generation appears in the dashboard's logs **scoped to that app**.

Acceptance: `cd dashboard && npx playwright test` → passes with the DB freshly recreated; the dashboard
issues **zero** `fetch()` outside `@litegen/sdk`.

# Phasing

| Phase | Scope | Gate |
|---|---|---|
| **1** | Multi-tenant identity & auth core (schema, mode switch, org/app/members/invites, id/secret keys, tenant-scoped queries, master-key hardening) **+ Rust integration tests + Playwright e2e** | **now** |
| **2** | BYO provider plumbing (encrypted per-app creds threaded per-request) + tests; provider-cred UI | after 1 green |
| **3** | Horizontal-scale state: Redis rate-limit/circuit-breaker/cache; poller `FOR UPDATE SKIP LOCKED` leasing; S3/Spaces artifacts | before multi-instance |
| **4** | Infra: DO Managed Postgres + Managed Redis, ≥2 app droplets behind a DO Load Balancer, TLS + DNS (`api.litegen.*`), extend `deploy.js`; dashboard on Cloudflare Pages | **explicit $ approval** |

# Implementation order (Phase 1)
1. **Schema + traits + backfill migration** (postgres + sqlite mirrors); `Organization`/`Application`/
   `Membership`/`ProviderCredential` types; `DatabaseStore` methods; unit tests for tenant-scoped queries
   and the backfill.
2. **Tenant context middleware**: `KeyContext.{org_id,app_id}`; membership validation for sessions; mode
   switch; master-key/dev-mode hardening.
3. **id/secret keys**: `public_id` + secret format, create/list/rotate/revoke scoped to active app.
4. **Org/app/member/invitation endpoints** + per-org permissions.
5. **Scope existing data endpoints** (keys/generations/logs/audit/stats/generate) by org_id+app_id.
6. **OpenAPI annotations** for all new endpoints → **SDK regen** (`sdks/scripts/regen-all.sh`), commit.
7. **Rust integration test suite** (`tests/multitenant_api.rs`) — the matrix above. (TDD: write
   failing tests alongside each endpoint where practical.)
8. **Dashboard**: org/app context + switcher, onboarding, scoped pages, Members/Org-settings, SDK header
   injection.
9. **Playwright e2e** (`multitenant.spec.ts`) + e2e-server harness (DB recreate each run).
10. **Polish**: clippy/build/lint clean; README "Hosting / multi-tenant" section; document
    `LITEGEN__MODE`, `LITEGEN__SECRETS_KEY`, `X-Litegen-Org-Id/App-Id`.

# Acceptance criteria
- `cargo test` (lib + `multitenant_api` integration) green; `cargo clippy --all-targets -- -D warnings`
  clean; `cargo build --release` clean.
- `cd dashboard && npm run build` clean; `cd sdks/typescript && npm run build && npm test` clean.
- `cd dashboard && npx playwright test` green with a freshly recreated DB; the multi-tenant flow +
  cross-tenant isolation are exercised through the UI.
- Existing single-tenant behavior (master-key Bearer god path, existing god test) still passes under
  `LITEGEN__MODE=single_tenant`.

# Security checklist (additions to the prior auth spec)
| Concern | Mitigation |
|---|---|
| Cross-tenant data leak | Every query filtered by `org_id`(+`app_id`); membership verified server-side before honoring `X-Litegen-Org-Id` |
| Master key as cross-tenant god | Hosted mode: master key = platform admin only, no implicit tenant data access |
| Dev "any token" bypass in prod | Disabled in hosted mode |
| BYO provider secret at rest | AES-256-GCM with `LITEGEN__SECRETS_KEY`; plaintext never returned; only `display_hint` shown |
| API secret at rest | `sha256` hash only (`key_hash`); secret shown once at creation |
| Tenant enumeration | Org/app ids are UUIDs; not-a-member → 403/404 without revealing existence |
| Invitation scoped to wrong org | Invitations carry `org_id`; accept joins exactly that org with the invited role |
```
