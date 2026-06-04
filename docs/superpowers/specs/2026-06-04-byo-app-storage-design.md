---
title: BYO per-app S3 storage for generated files (orgs/apps bring their own bucket)
status: approved
owner: joe
date: 2026-06-04
---

# Goal

Let each **Application** in the hosted multi-tenant platform bring its own **S3-compatible bucket**
for storing generated image files, instead of every tenant sharing the single platform-wide bucket
configured at deploy time. This is the storage half of the multi-tenant "bring-your-own" story:
[Phase 2](2026-06-03-multi-tenant-hosted-platform-design.md) shipped BYO **provider** credentials;
this spec adds BYO **storage** credentials, mirroring that vertical end to end.

When an app has configured its own storage, its generated images upload to the app's bucket and the
returned URL points there. When it hasn't, behavior is unchanged: fall back to the global
platform store (env/config S3 if set, else base64 inline).

This is the object-storage slice of "Phase 3" from the platform spec, scoped down to per-tenant BYO
storage (the Redis/poller-leasing horizontal-scale items remain separate).

# Decisions (locked with user)

1. **Scope: image-generation output only.** The reference-image **materializer** (currently a no-op
   `LocalStorage`) and **video** outputs (provider-hosted; we only persist the provider URL in
   `generations`) are explicitly **out of scope** for this slice.
2. **Storage model: hybrid.** Non-secret fields (`bucket_name`, `region`, `endpoint_url`,
   `custom_public_url`, `path_prefix`) are stored as **plaintext columns** so the dashboard can show
   and edit them. Only the credential pair (`access_key_id` + `secret_access_key`) is **encrypted at
   rest** (AES-256-GCM, the same `LITEGEN__SECRETS_KEY` used by provider creds).
3. **Fallback: global platform store.** An app with no BYO storage configured behaves exactly as
   today — uses the global configured `ImageStore` (S3 if set, else base64 inline). BYO storage is
   purely additive/opt-in. No regression for single-tenant deploys.
4. **Threading: resolve-in-handler override (Approach A).** Mirror the existing `app_creds` pattern:
   the handler resolves an `Option<Arc<dyn ImageStore>>` from the tenant context and passes it down;
   the router falls back to its global `image_store`. The router never gains a DB or secrets-key
   dependency.
5. **One storage config per app.** `app_storage_credentials.app_id` is the primary key. A `backend`
   column (default `'s3'`) leaves room for `r2`/`gcs` later without a schema change.
6. **End-to-end parity.** Backend + SDK regen + dashboard UI + **thorough Rust integration tests and
   Playwright e2e tests** all land in this slice (see Testing — hard requirement).

# Non-goals (this spec)

- Per-app storage for the reference-image **materializer** and **video** outputs (out of scope above).
- Multiple storage backends per app, or non-S3 backends (`r2`/`gcs`) — schema is forward-compatible
  but only `s3` is implemented.
- Migrating existing globally-stored images into per-app buckets (no backfill of objects).
- Redis-backed rate-limit/circuit-breaker, poller leasing — the other Phase 3 items.
- A per-app store cache (premature; `S3Store` construction is local and cheap — build per request).

# Architecture overview

```
POST /v1/images/generations  (Bearer sk_live_… resolves {org_id, app_id})
  └─ generate_image handler
       ├─ resolve_app_credential(state, key_ctx, provider)   → Option<ProviderCredentials>   (existing)
       ├─ resolve_app_image_store(state, key_ctx)            → Option<Arc<dyn ImageStore>>   (NEW)
       └─ router.generate_image(schema, base, extras, materialized, app_creds, app_store)    (NEW param)
            └─ build_image_results(output, extras, app_store.unwrap_or(&self.image_store))    (fallback)
                 └─ ImageStore::store(bytes, content_type, generation_id) → URL
```

`resolve_app_image_store` reads `key_ctx.app_id`, loads the app's `app_storage_credentials` row,
decrypts the credential pair, builds an `ImageStorageConfig`, and constructs an `S3Store`. Any
miss (no row, no secrets key, no app_id) → `None` → router uses the global store. Any error on a row
that *does* exist (decrypt/build failure) → log a warning and return `None` (fall back; never fail the
generation).

# Data model

New table `app_storage_credentials`, **mirrored in `migrations/postgres` + `migrations/sqlite`**,
numbered after the existing series. One row per app.

### `app_storage_credentials`  (`…0009_app_storage.sql`)

| Column | Type (pg) | Notes |
|---|---|---|
| `app_id` | TEXT PK REFERENCES applications(id) | one storage config per app |
| `backend` | TEXT NOT NULL DEFAULT 's3' | future: 'r2','gcs' (only 's3' implemented) |
| `bucket_name` | TEXT NOT NULL | plaintext |
| `region` | TEXT NOT NULL DEFAULT 'us-east-1' | plaintext |
| `endpoint_url` | TEXT NULL | MinIO/R2/Spaces custom endpoint |
| `custom_public_url` | TEXT NULL | CDN / custom domain base |
| `path_prefix` | TEXT NULL | default `litegen/images` when null |
| `secret_ciphertext` | TEXT NOT NULL | base64 AES-256-GCM of `{"access_key_id":…,"secret_access_key":…}` |
| `secret_nonce` | TEXT NOT NULL | base64 12-byte GCM nonce |
| `created_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | (sqlite: TEXT, matching existing convention) |
| `updated_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | |

Notes:
- FK to `applications(id)` keeps the row tenant-scoped; deleting an app should not orphan storage
  config (delete cascades at the app layer / handler, matching how provider creds are handled).
- Both credential fields go in one encrypted JSON blob → a single `(ciphertext, nonce)` pair, exactly
  like `provider_credentials` encrypts its JSON. The encryption key comes from `LITEGEN__SECRETS_KEY`
  (base64 32 bytes), required in hosted mode.
- The access-key-id **last-4** is derived at write time and returned by GET as `access_key_id_hint`;
  the raw access key id and the secret are **never** returned by any endpoint.

# DB layer

`DatabaseStore` (`db/trait_def.rs`) gains three methods, implemented in `db/postgres.rs` and
`db/sqlite.rs`:

```rust
async fn upsert_app_storage(&self, row: &AppStorageRow) -> Result<(), sqlx::Error>;
async fn get_app_storage(&self, app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error>;
async fn delete_app_storage(&self, app_id: &str) -> Result<(), sqlx::Error>;
```

`AppStorageRow` (in `types/`) carries the plaintext fields + `secret_ciphertext`/`secret_nonce` +
timestamps. `upsert_app_storage` does an `INSERT … ON CONFLICT (app_id) DO UPDATE` (pg) /
`INSERT OR REPLACE` (sqlite), bumping `updated_at`. Decryption happens in the resolution/handler
layer, never in the DB layer (mirrors provider creds).

# Encryption

Reuse `auth::secrets::{encrypt, decrypt}` (AES-256-GCM, random 12-byte nonce, base64 out) and the
`AppState.secrets_key: Option<[u8;32]>` decoded from `LITEGEN__SECRETS_KEY`. The encrypted plaintext
is `serde_json::to_vec(&{ "access_key_id": String, "secret_access_key": String })`. No new crates.

# API endpoints

Single config per app, so a single resource (no `{provider}` path segment like provider creds had).
All under the existing app-scoped router; all annotated with `#[utoipa::path]`.

### `PUT /v1/apps/{app_id}/storage` — upsert
Permission `storage_cred:write`. Request:
```json
{
  "backend": "s3",
  "bucket_name": "my-app-bucket",
  "region": "us-east-1",
  "endpoint_url": "https://minio.example.com:9000",   // optional
  "custom_public_url": "https://cdn.example.com",      // optional
  "path_prefix": "litegen/images",                     // optional
  "access_key_id": "AKIA…",                            // write-only
  "secret_access_key": "…"                             // write-only
}
```
Validation: `bucket_name` required and non-empty; `access_key_id` + `secret_access_key` both required
on first create (on update, if both omitted the existing encrypted pair is retained; if one is
present both must be). `400 secrets_key_unavailable` if `secrets_key` is not configured (hosted
requires it). Returns the GET shape (200).

### `GET /v1/apps/{app_id}/storage` — read (no secret)
Permission `storage_cred:read`. Returns:
```json
{
  "configured": true,
  "backend": "s3",
  "bucket_name": "my-app-bucket",
  "region": "us-east-1",
  "endpoint_url": "https://minio.example.com:9000",
  "custom_public_url": "https://cdn.example.com",
  "path_prefix": "litegen/images",
  "access_key_id_hint": "…AB12",
  "updated_at": "2026-06-04T…Z"
}
```
When no row exists: `{ "configured": false }` (200). **Never** returns `access_key_id` or
`secret_access_key`.

### `DELETE /v1/apps/{app_id}/storage` — remove
Permission `storage_cred:delete`. Removes the row → app reverts to the global-store fallback. 204.

All three first enforce that the caller is a member of the app's org with the required permission, and
that the app belongs to the caller's active org (cross-tenant attempts → `403`/`404`, matching the
existing app-scoped handlers).

# Permissions

Add to `auth/permissions.rs`, role-mapped exactly like `provider_cred:*`:
```
storage_cred:read    storage_cred:write    storage_cred:delete
```
Owner/Admin/Member → read+write+delete; Viewer → read only.

# Resolution & threading

`api/handlers/mod.rs`:
```rust
async fn resolve_app_image_store(
    state: &AppState,
    key_ctx: &Option<KeyContext>,
) -> Option<Arc<dyn ImageStore>> {
    let app_id = key_ctx.as_ref()?.app_id.as_deref()?;
    let secrets_key = state.secrets_key?;
    let row = state.db.get_app_storage(app_id).await.ok()??;
    let plaintext = crate::auth::secrets::decrypt(&secrets_key, &row.secret_ciphertext, &row.secret_nonce)
        .map_err(|e| warn!(app_id, error=%e, "byo storage: decrypt failed, falling back to global")).ok()?;
    let secret: StorageSecret = serde_json::from_slice(&plaintext)
        .map_err(|e| warn!(app_id, error=%e, "byo storage: corrupt secret, falling back to global")).ok()?;
    let cfg = ImageStorageConfig {
        backend: row.backend,                       // "s3"
        path_prefix: row.path_prefix,
        s3: Some(S3StorageConfig {
            bucket_name: row.bucket_name,
            region: row.region,
            endpoint_url: row.endpoint_url,
            custom_public_url: row.custom_public_url,
            access_key_id: Some(secret.access_key_id),
            secret_access_key: Some(secret.secret_access_key),
        }),
    };
    match S3Store::from_config(&cfg) {
        Ok(store) => Some(Arc::new(store) as Arc<dyn ImageStore>),
        Err(e) => { warn!(app_id, error=%e, "byo storage: build failed, falling back to global"); None }
    }
}
```

`generate_image` handler calls this alongside `resolve_app_credential` and passes the result into a
new last param of `ProxyRouter::generate_image(..., app_store: Option<Arc<dyn ImageStore>>)`.
`build_image_results` takes the store by reference and the router supplies
`app_store.as_ref().unwrap_or(&self.image_store)`. No other call site changes.

`S3Store::from_config` is reused as-is (it already supports custom endpoint + path-style + custom
public URL), so the per-app code path and the global code path build identical store types.

# SDK

`#[utoipa::path]` on the three endpoints → regenerate via `sdks/scripts/regen-all.sh` → typed client
methods `getAppStorage(appId)`, `putAppStorage(appId, body)`, `deleteAppStorage(appId)` in the
TS (and Python) SDKs. The dashboard consumes only the SDK (no raw `fetch`).

# Dashboard

A **Storage** section in app settings (sibling to the provider-credentials UI):
- Form fields: bucket, region, endpoint URL, custom public URL, path prefix, access key id, secret
  access key (the last two are write-only password inputs).
- On load, GET populates the non-secret fields and shows `configured` + `access_key_id_hint` instead
  of the secret. Saving issues `PUT`; a "Remove storage" action issues `DELETE`.
- Rendering/edit gated by the active org's `storage_cred` permissions (read-only for Viewer).

# Testing strategy (HARD REQUIREMENT)

Every piece below ships with tests. "Done" = the matrices green.

## Rust integration — extend `litegen-core/tests/multitenant_api.rs`
Real server, real HTTP, fresh DB per test, `LITEGEN__MODE=hosted` with a `LITEGEN__SECRETS_KEY` set,
mock provider for generation, and a **`wiremock` server standing in for S3** (the app's
`endpoint_url` points at it, path-style). Each an isolated `#[tokio::test]`:

1. **CRUD round-trip**: `PUT` storage on an app → `GET` returns the non-secret config +
   `access_key_id_hint` + `configured:true`; the response (and a raw body scan) **never** contains the
   secret or the full access key id. `DELETE` → `GET` returns `configured:false`.
2. **Upsert semantics**: a second `PUT` updates bucket/region and bumps `updated_at`; a `PUT` that
   omits both key fields **retains** the existing encrypted pair (generation still uploads); a `PUT`
   with only one of the two key fields → `400`.
3. **Secret encrypted at rest**: after a `PUT`, query the DB row directly and assert
   `secret_ciphertext`/`secret_nonce` are present and the plaintext access/secret keys appear **nowhere**
   in the stored columns.
4. **Per-app upload (the crux)**: configure app A's storage at the wiremock S3 endpoint; `POST
   /v1/images/generations` with `Bearer sk_live_…` (mock provider returns image bytes) → 200; assert
   **wiremock received a `PUT`** to `/{bucket}/{path_prefix}/{generation_id}.{ext}`, and the returned
   image result `url` points at the app's bucket/custom_public_url (not the global store).
5. **Fallback when unconfigured**: an app with **no** storage row generates an image → no request hits
   the per-app wiremock; result uses the global store behavior (URL from global bucket if the test
   configures one, else `b64_json` inline). Confirms additive/opt-in semantics.
6. **Fallback on corrupt config**: hand-write a row with a bogus `secret_ciphertext` → generation
   still returns 200 via the global fallback (logs a warning; does not 500), and the per-app wiremock
   is **not** hit.
7. **`secrets_key` required**: boot a server with **no** `LITEGEN__SECRETS_KEY`; `PUT` storage →
   `400 secrets_key_unavailable`.
8. **Cross-tenant isolation**: org B's session/key cannot `GET`/`PUT`/`DELETE` org A's app storage
   (`403`/`404`); org B sending `X-Litegen-App-Id:<appA>` (not a member) → `403`. Org A's bucket is
   never used for an org B generation.
9. **Permission gating**: a Viewer member can `GET` but `PUT`/`DELETE` → `403 forbidden_permission`;
   Member/Admin/Owner can write+delete.
10. **Delete reverts**: configure storage, confirm per-app upload (test 4), `DELETE`, generate again →
    falls back to global (per-app wiremock not hit).

Acceptance: `cargo test --test multitenant_api` green; existing `cargo test` stays green;
`cargo clippy --all-targets -- -D warnings` clean.

## Playwright e2e — extend `dashboard/e2e-mt/multitenant.spec.ts` (DB recreated each run)
Driven entirely through the dashboard UI on `@litegen/sdk` (zero raw `fetch`):

1. **Configure storage via UI**: sign up org A → app settings → Storage section → fill
   bucket/region/endpoint/custom URL/path prefix + access key id + secret → Save → success.
2. **Reload shows config sans secret**: refresh → non-secret fields populated; `configured` shown;
   the secret input is empty/masked and the `access_key_id_hint` (…last4) is displayed; the raw secret
   never appears in the DOM or any network response.
3. **Generate writes to the app bucket**: with storage pointed at the e2e mock S3 endpoint, run a
   generation (Playground UI or an in-test `request.post` with the minted key) → the dashboard's
   generation/logs entry shows a URL pointing at the app bucket.
4. **Remove storage**: click "Remove storage" → confirm → fields cleared / `configured:false`.
5. **Isolation**: in a second browser context sign up org B; org B's app settings show **no** storage
   config and cannot read org A's (the switcher never exposes org A's app).
6. **Role gating**: a Viewer teammate sees the Storage config read-only (no Save/Remove).

Acceptance: `cd dashboard && npx playwright test` (the multitenant config) green with a freshly
recreated DB.

## Other gates
- `cd dashboard && npm run build` clean; `cd sdks/typescript && npm run build && npm test` clean
  (SDK exposes the new methods); regenerated SDK committed.
- `cargo build --release` clean.

# Implementation order
1. **Migration** (`…0009_app_storage.sql`, pg + sqlite mirrors) + `AppStorageRow` type +
   `DatabaseStore` methods (both impls) + DB-level unit tests (upsert/get/delete, ON CONFLICT).
2. **Permissions** (`storage_cred:read|write|delete`) + role mapping + tests.
3. **API handlers** (`PUT`/`GET`/`DELETE /v1/apps/{app_id}/storage`) with encryption on write,
   hint derivation, validation, and tenant/permission guards. Integration tests 1–3, 7–9.
4. **Resolution & threading** (`resolve_app_image_store`, new `generate_image` router param,
   `build_image_results` fallback). Integration tests 4–6, 10 (with the wiremock S3 harness).
5. **OpenAPI annotations → SDK regen** (`sdks/scripts/regen-all.sh`), commit regenerated SDKs.
6. **Dashboard** Storage settings section (form, read-without-secret, delete, permission gating).
7. **Playwright e2e** (`multitenant.spec.ts`) — the UI matrix above.
8. **Polish**: clippy/build/lint clean; README "BYO storage" note documenting the endpoints and
   `LITEGEN__SECRETS_KEY` dependency.

# Acceptance criteria
- `cargo test` (lib + `multitenant_api`) green; `cargo clippy --all-targets -- -D warnings` clean;
  `cargo build --release` clean.
- `cd dashboard && npm run build` clean; `cd sdks/typescript && npm run build && npm test` clean.
- `cd dashboard && npx playwright test` (multitenant config) green with a freshly recreated DB; the
  storage configure → upload → isolation flow is exercised through the UI.
- Existing single-tenant + multi-tenant behavior unchanged when no app configures BYO storage.

# Security checklist
| Concern | Mitigation |
|---|---|
| BYO storage secret at rest | `secret_access_key` + `access_key_id` encrypted as one AES-256-GCM blob with `LITEGEN__SECRETS_KEY`; plaintext never returned (only `access_key_id_hint`) |
| Cross-tenant storage access | Every storage endpoint membership- + permission-checked and app-belongs-to-org verified; generation resolves the store strictly from the request's own `app_id` |
| Wrong-bucket leak on corrupt config | Decrypt/build failure on an existing row → log + fall back to global; never silently uploads tenant data to an unintended per-app target |
| Secret echoed back | `GET` returns no secret/access-key-id; raw-body scans asserted in integration test 1 |
| Missing encryption key in hosted | `PUT` → `400 secrets_key_unavailable` rather than storing plaintext |
| Storage enumeration | App ids are UUIDs; not-a-member → `403`/`404` without revealing existence |
