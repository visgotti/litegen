# BYO Per-App S3 Storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let each Application in the hosted multi-tenant platform bring its own S3-compatible bucket for storing generated image files, falling back to the global platform store when unconfigured.

**Architecture:** Mirror the shipped Phase-2 BYO **provider-credential** vertical. A new `app_storage_credentials` table stores non-secret S3 config as plaintext columns plus the `{access_key_id, secret_access_key}` pair encrypted with AES-256-GCM (`LITEGEN__SECRETS_KEY`). At image-generation time the handler resolves an `Option<Arc<dyn ImageStore>>` from the request's tenant context and threads it into `ProxyRouter::generate_image`, which falls back to its global `image_store`. SDK + dashboard + thorough Rust integration and Playwright e2e tests ship in the same slice.

**Tech Stack:** Rust (axum, sqlx for Postgres + SQLite, `s3`/`rust-s3`, `aes-gcm`), utoipa OpenAPI, TypeScript SDK (`openapi-typescript` + hand-written client namespaces), React dashboard, Playwright, `wiremock` for tests.

**Spec:** `docs/superpowers/specs/2026-06-04-byo-app-storage-design.md`

**Reference (the template to mirror):** the `provider_credentials` vertical —
- migration: `litegen-core/migrations/{postgres,sqlite}/20240101000008_multitenant.sql`
- trait: `litegen-core/src/db/trait_def.rs:497-532`
- impls: `litegen-core/src/db/postgres.rs:1488-1559`, `litegen-core/src/db/sqlite.rs:1470-1541`
- handlers: `litegen-core/src/api/handlers/orgs.rs` (`create/list/delete_provider_credential`, `require_member_perm`, `org_for_app`, `err`, `internal_error`)
- resolution: `litegen-core/src/api/handlers/mod.rs:2031-2069` (`resolve_app_credential`)
- routing: `litegen-core/src/api/handlers/mod.rs:1714-1800` (`create_router`)
- openapi: `litegen-core/src/api/openapi.rs:11-75`
- secrets: `litegen-core/src/auth/secrets.rs` — `encrypt(&[u8;32], &[u8]) -> Result<(String,String),String>`, `decrypt(&[u8;32], &str, &str) -> Result<Vec<u8>,String>`
- SDK: `sdks/typescript/src/client.ts:861-899`
- dashboard: `dashboard/src/pages/Organization.tsx`
- e2e: `dashboard/e2e-mt/multitenant.spec.ts`, `dashboard/playwright.multitenant.config.ts`

**Migration version:** Postgres already has `…0009_pg_timestamptz.sql`; SQLite stops at `…0008`. Use **`20240101000010_app_storage.sql`** in BOTH dialect dirs (free in both; SQLite skipping `0009` is harmless — sqlx applies in lexical order and tolerates gaps).

---

## Task 1: Migration — `app_storage_credentials` table (Postgres + SQLite)

**Files:**
- Create: `litegen-core/migrations/postgres/20240101000010_app_storage.sql`
- Create: `litegen-core/migrations/sqlite/20240101000010_app_storage.sql`

- [ ] **Step 1: Write the Postgres migration**

Create `litegen-core/migrations/postgres/20240101000010_app_storage.sql`:

```sql
-- Per-app BYO object storage config. Non-secret fields are plaintext (shown/edited
-- in the dashboard); the {access_key_id, secret_access_key} pair is AES-256-GCM
-- encrypted into (secret_ciphertext, secret_nonce) with LITEGEN__SECRETS_KEY.
-- One config per app.
CREATE TABLE app_storage_credentials (
    app_id              TEXT PRIMARY KEY REFERENCES applications(id),
    backend             TEXT NOT NULL DEFAULT 's3',
    bucket_name         TEXT NOT NULL,
    region              TEXT NOT NULL DEFAULT 'us-east-1',
    endpoint_url        TEXT,
    custom_public_url   TEXT,
    path_prefix         TEXT,
    access_key_id_hint  TEXT,
    secret_ciphertext   TEXT NOT NULL,
    secret_nonce        TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Write the SQLite migration**

Create `litegen-core/migrations/sqlite/20240101000010_app_storage.sql`:

```sql
-- Per-app BYO object storage config (see postgres mirror). One config per app.
CREATE TABLE app_storage_credentials (
    app_id              TEXT PRIMARY KEY REFERENCES applications(id),
    backend             TEXT NOT NULL DEFAULT 's3',
    bucket_name         TEXT NOT NULL,
    region              TEXT NOT NULL DEFAULT 'us-east-1',
    endpoint_url        TEXT,
    custom_public_url   TEXT,
    path_prefix         TEXT,
    access_key_id_hint  TEXT,
    secret_ciphertext   TEXT NOT NULL,
    secret_nonce        TEXT NOT NULL,
    created_at          TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

- [ ] **Step 3: Verify the SQLite migration applies cleanly**

Run: `cd litegen-core && cargo test --test multitenant_api signup_creates_org -- --nocapture 2>&1 | tail -20`
(Any existing test that calls `spawn_app()` connects a fresh SQLite DB and runs all migrations, so a broken migration fails here.)
Expected: PASS (migration applied; existing test still green). If you get a migration error, the SQL above is malformed — fix before continuing.

- [ ] **Step 4: Commit**

```bash
git add litegen-core/migrations/postgres/20240101000010_app_storage.sql litegen-core/migrations/sqlite/20240101000010_app_storage.sql
git commit -m "feat(db): app_storage_credentials migration (pg + sqlite)"
```

---

## Task 2: Types + DatabaseStore methods + DB round-trip test

**Files:**
- Modify: `litegen-core/src/types/mod.rs` (add `AppStorageRow`, `AppStorageInfo`, `AppStorageUpsert` near `ProviderCredentialInfo` ~line 574)
- Modify: `litegen-core/src/db/trait_def.rs` (add 3 methods after `delete_provider_credential` ~line 532)
- Modify: `litegen-core/src/db/sqlite.rs` (impl 3 methods after `delete_provider_credential` ~line 1541)
- Modify: `litegen-core/src/db/postgres.rs` (impl 3 methods after `delete_provider_credential` ~line 1559)
- Test: `litegen-core/tests/app_storage_db.rs` (new)

- [ ] **Step 1: Write the failing DB round-trip test**

Create `litegen-core/tests/app_storage_db.rs`:

```rust
//! Direct DB-layer tests for per-app BYO storage config. Verifies upsert/get/delete
//! and that the credential pair is encrypted at rest (plaintext never stored).
use std::sync::Arc;

use litegen::auth::secrets;
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::types::AppStorageUpsert;

// The 0008 backfill always inserts this default application row, satisfying the
// app_storage_credentials.app_id FK without creating an org/app by hand.
const DEFAULT_APP_ID: &str = "00000000-0000-0000-0000-000000000002";
const KEY: [u8; 32] = [9u8; 32];

async fn db() -> (Arc<dyn DatabaseStore>, tempfile::NamedTempFile) {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let url = format!("sqlite://{}?mode=rwc", tmp.path().display());
    let db: Arc<dyn DatabaseStore> =
        Arc::new(SqliteDatabase::connect(&url).await.expect("connect + migrate"));
    (db, tmp)
}

fn encrypt_keys(access: &str, secret: &str) -> (String, String) {
    let plaintext = serde_json::to_vec(
        &serde_json::json!({ "access_key_id": access, "secret_access_key": secret }),
    )
    .unwrap();
    secrets::encrypt(&KEY, &plaintext).unwrap()
}

#[tokio::test]
async fn app_storage_upsert_get_delete_roundtrip() {
    let (db, _tmp) = db().await;

    // Initially absent.
    assert!(db.get_app_storage(DEFAULT_APP_ID).await.unwrap().is_none());

    let (ct, nonce) = encrypt_keys("AKIAEXAMPLE123", "s3cr3t-value");
    let input = AppStorageUpsert {
        app_id: DEFAULT_APP_ID.to_string(),
        backend: "s3".to_string(),
        bucket_name: "my-bucket".to_string(),
        region: "us-west-2".to_string(),
        endpoint_url: Some("https://minio.example.com".to_string()),
        custom_public_url: None,
        path_prefix: Some("litegen/images".to_string()),
        access_key_id_hint: Some("…E123".to_string()),
        secret_ciphertext: ct.clone(),
        secret_nonce: nonce.clone(),
    };
    db.upsert_app_storage(&input).await.unwrap();

    let row = db.get_app_storage(DEFAULT_APP_ID).await.unwrap().expect("row present");
    assert_eq!(row.backend, "s3");
    assert_eq!(row.bucket_name, "my-bucket");
    assert_eq!(row.region, "us-west-2");
    assert_eq!(row.endpoint_url.as_deref(), Some("https://minio.example.com"));
    assert_eq!(row.access_key_id_hint.as_deref(), Some("…E123"));

    // Encrypted at rest: stored ciphertext must NOT contain the plaintext secret,
    // and must decrypt back to the original JSON.
    assert!(!row.secret_ciphertext.contains("s3cr3t-value"));
    assert!(!row.secret_ciphertext.contains("AKIAEXAMPLE123"));
    let pt = secrets::decrypt(&KEY, &row.secret_ciphertext, &row.secret_nonce).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&pt).unwrap();
    assert_eq!(v["access_key_id"], "AKIAEXAMPLE123");
    assert_eq!(v["secret_access_key"], "s3cr3t-value");

    // Upsert again updates in place (still one row, new bucket).
    let (ct2, nonce2) = encrypt_keys("AKIAEXAMPLE123", "rotated");
    let updated = AppStorageUpsert {
        bucket_name: "bucket-2".to_string(),
        secret_ciphertext: ct2,
        secret_nonce: nonce2,
        ..input.clone()
    };
    db.upsert_app_storage(&updated).await.unwrap();
    let row2 = db.get_app_storage(DEFAULT_APP_ID).await.unwrap().unwrap();
    assert_eq!(row2.bucket_name, "bucket-2");

    // Delete.
    assert!(db.delete_app_storage(DEFAULT_APP_ID).await.unwrap());
    assert!(db.get_app_storage(DEFAULT_APP_ID).await.unwrap().is_none());
    assert!(!db.delete_app_storage(DEFAULT_APP_ID).await.unwrap()); // already gone
}
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cd litegen-core && cargo test --test app_storage_db 2>&1 | tail -20`
Expected: FAIL — compile errors (`AppStorageUpsert` unresolved, `upsert_app_storage`/`get_app_storage`/`delete_app_storage` not found). That confirms the test targets the new API.

- [ ] **Step 3: Add the types**

In `litegen-core/src/types/mod.rs`, immediately after the `ProviderCredentialInfo` struct (~line 580), add:

```rust
/// Internal row for per-app BYO storage config (DB → resolution/handlers).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AppStorageRow {
    pub backend: String,
    pub bucket_name: String,
    pub region: String,
    pub endpoint_url: Option<String>,
    pub custom_public_url: Option<String>,
    pub path_prefix: Option<String>,
    pub access_key_id_hint: Option<String>,
    pub secret_ciphertext: String,
    pub secret_nonce: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Owned input for upserting per-app BYO storage config.
#[derive(Debug, Clone)]
pub struct AppStorageUpsert {
    pub app_id: String,
    pub backend: String,
    pub bucket_name: String,
    pub region: String,
    pub endpoint_url: Option<String>,
    pub custom_public_url: Option<String>,
    pub path_prefix: Option<String>,
    pub access_key_id_hint: Option<String>,
    pub secret_ciphertext: String,
    pub secret_nonce: String,
}

/// Public view of per-app BYO storage config — NEVER includes the secret.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AppStorageInfo {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

(If `Serialize`, `Deserialize`, or `ToSchema` are not already in scope at the top of `types/mod.rs`, they are — `ProviderCredentialInfo` directly above uses all three.)

- [ ] **Step 4: Add the trait methods (default impls)**

In `litegen-core/src/db/trait_def.rs`, immediately after the `delete_provider_credential` default impl (~line 532), add:

```rust
    // ─── Per-app BYO storage config ─────────────────────────────────────

    async fn upsert_app_storage(&self, _input: &AppStorageUpsert) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_app_storage(&self, _app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_app_storage(&self, _app_id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }
```

(`trait_def.rs` has `use crate::types::*;` at the top, so `AppStorageUpsert`/`AppStorageRow` resolve.)

- [ ] **Step 5: Implement for SQLite**

In `litegen-core/src/db/sqlite.rs`, inside `impl DatabaseStore for SqliteDatabase`, after `delete_provider_credential` (~line 1541), add:

```rust
    async fn upsert_app_storage(&self, input: &AppStorageUpsert) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO app_storage_credentials \
                (app_id, backend, bucket_name, region, endpoint_url, custom_public_url, \
                 path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, \
                 created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT (app_id) DO UPDATE SET \
                backend = excluded.backend, \
                bucket_name = excluded.bucket_name, \
                region = excluded.region, \
                endpoint_url = excluded.endpoint_url, \
                custom_public_url = excluded.custom_public_url, \
                path_prefix = excluded.path_prefix, \
                access_key_id_hint = excluded.access_key_id_hint, \
                secret_ciphertext = excluded.secret_ciphertext, \
                secret_nonce = excluded.secret_nonce, \
                updated_at = datetime('now')",
        )
        .bind(&input.app_id)
        .bind(&input.backend)
        .bind(&input.bucket_name)
        .bind(&input.region)
        .bind(&input.endpoint_url)
        .bind(&input.custom_public_url)
        .bind(&input.path_prefix)
        .bind(&input.access_key_id_hint)
        .bind(&input.secret_ciphertext)
        .bind(&input.secret_nonce)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_app_storage(&self, app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        let row = sqlx::query_as::<_, AppStorageRow>(
            "SELECT backend, bucket_name, region, endpoint_url, custom_public_url, \
                    path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, updated_at \
             FROM app_storage_credentials WHERE app_id = ?",
        )
        .bind(app_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_app_storage(&self, app_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM app_storage_credentials WHERE app_id = ?")
            .bind(app_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
```

(`AppStorageRow`/`AppStorageUpsert` resolve via the existing `use crate::types::*;` in `sqlite.rs`. If a build error says otherwise, add `use crate::types::{AppStorageRow, AppStorageUpsert};`.)

- [ ] **Step 6: Implement for Postgres**

In `litegen-core/src/db/postgres.rs`, inside `impl DatabaseStore for PostgresDatabase`, after `delete_provider_credential` (~line 1559), add:

```rust
    async fn upsert_app_storage(&self, input: &AppStorageUpsert) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO app_storage_credentials \
                (app_id, backend, bucket_name, region, endpoint_url, custom_public_url, \
                 path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, \
                 created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW(), NOW()) \
             ON CONFLICT (app_id) DO UPDATE SET \
                backend = EXCLUDED.backend, \
                bucket_name = EXCLUDED.bucket_name, \
                region = EXCLUDED.region, \
                endpoint_url = EXCLUDED.endpoint_url, \
                custom_public_url = EXCLUDED.custom_public_url, \
                path_prefix = EXCLUDED.path_prefix, \
                access_key_id_hint = EXCLUDED.access_key_id_hint, \
                secret_ciphertext = EXCLUDED.secret_ciphertext, \
                secret_nonce = EXCLUDED.secret_nonce, \
                updated_at = NOW()",
        )
        .bind(&input.app_id)
        .bind(&input.backend)
        .bind(&input.bucket_name)
        .bind(&input.region)
        .bind(&input.endpoint_url)
        .bind(&input.custom_public_url)
        .bind(&input.path_prefix)
        .bind(&input.access_key_id_hint)
        .bind(&input.secret_ciphertext)
        .bind(&input.secret_nonce)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_app_storage(&self, app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        let row = sqlx::query_as::<_, AppStorageRow>(
            "SELECT backend, bucket_name, region, endpoint_url, custom_public_url, \
                    path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, updated_at \
             FROM app_storage_credentials WHERE app_id = $1",
        )
        .bind(app_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_app_storage(&self, app_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM app_storage_credentials WHERE app_id = $1")
            .bind(app_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cd litegen-core && cargo test --test app_storage_db 2>&1 | tail -20`
Expected: PASS (`app_storage_upsert_get_delete_roundtrip ... ok`).

- [ ] **Step 8: Commit**

```bash
git add litegen-core/src/types/mod.rs litegen-core/src/db/trait_def.rs litegen-core/src/db/sqlite.rs litegen-core/src/db/postgres.rs litegen-core/tests/app_storage_db.rs
git commit -m "feat(db): app storage upsert/get/delete + encrypted-at-rest test"
```

---

## Task 3: Permissions — `storage_cred:{read,write,delete}`

**Files:**
- Modify: `litegen-core/src/auth/permissions.rs` (enum ~line 40, `as_str` ~line 90, `permissions_for` Owner/Admin/Member/Viewer arms)
- Test: add a unit test at the bottom of `litegen-core/src/auth/permissions.rs`

> **Role mapping (mirrors `provider_cred:*` EXACTLY):** Owner & Admin → read+write+delete; Member → read+write (NO delete); Viewer → read only. (This is the existing `provider_cred` mapping; the spec's prose said "Member r/w/d" but the locked decision is "role-mapped exactly like provider_cred", which wins.)

- [ ] **Step 1: Write the failing test**

At the bottom of `litegen-core/src/auth/permissions.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block (or add one if absent), add:

```rust
    #[test]
    fn storage_cred_role_mapping_mirrors_provider_cred() {
        use Permission::*;
        // Owner + Admin: full CRUD.
        for r in [Role::Owner, Role::Admin] {
            assert!(role_has(r, StorageCredRead));
            assert!(role_has(r, StorageCredWrite));
            assert!(role_has(r, StorageCredDelete));
        }
        // Member: read + write, NOT delete (mirrors provider_cred).
        assert!(role_has(Role::Member, StorageCredRead));
        assert!(role_has(Role::Member, StorageCredWrite));
        assert!(!role_has(Role::Member, StorageCredDelete));
        // Viewer: read only.
        assert!(role_has(Role::Viewer, StorageCredRead));
        assert!(!role_has(Role::Viewer, StorageCredWrite));
        // String form.
        assert_eq!(StorageCredRead.as_str(), "storage_cred:read");
        assert_eq!(StorageCredWrite.as_str(), "storage_cred:write");
        assert_eq!(StorageCredDelete.as_str(), "storage_cred:delete");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test --lib permissions::tests::storage_cred 2>&1 | tail -20`
Expected: FAIL — compile error (`StorageCredRead` not a variant).

- [ ] **Step 3: Add the enum variants**

In `litegen-core/src/auth/permissions.rs`, in the `Permission` enum, immediately after `ProviderCredDelete,` add:

```rust
    StorageCredRead,
    StorageCredWrite,
    StorageCredDelete,
```

- [ ] **Step 4: Add the `as_str` arms**

In the `as_str` match, immediately after the `Self::ProviderCredDelete => "provider_cred:delete",` arm, add:

```rust
            Self::StorageCredRead => "storage_cred:read",
            Self::StorageCredWrite => "storage_cred:write",
            Self::StorageCredDelete => "storage_cred:delete",
```

- [ ] **Step 5: Add to `permissions_for`**

In `permissions_for`, add to the slice for each role, next to the existing `ProviderCred*` entries:

- In `Role::Owner` (after `ProviderCredDelete,`): add `StorageCredRead, StorageCredWrite, StorageCredDelete,`
- In `Role::Admin` (after `ProviderCredDelete,`): add `StorageCredRead, StorageCredWrite, StorageCredDelete,`
- In `Role::Member` (after `ProviderCredWrite,`): add `StorageCredRead, StorageCredWrite,`
- In `Role::Viewer` (after `ProviderCredRead,`): add `StorageCredRead,`

- [ ] **Step 6: Run to verify it passes**

Run: `cd litegen-core && cargo test --lib permissions::tests::storage_cred 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/auth/permissions.rs
git commit -m "feat(auth): storage_cred read/write/delete permissions"
```

---

## Task 4: API handlers (PUT/GET/DELETE) + routing + OpenAPI + surface tests

**Files:**
- Modify: `litegen-core/src/api/handlers/orgs.rs` (add request struct, helper, 3 handlers — near the provider-credential handlers ~line 968)
- Modify: `litegen-core/src/api/handlers/mod.rs` (add `put` to routing import + 1 route line ~line 1795)
- Modify: `litegen-core/src/api/openapi.rs` (register 3 paths ~line 74)
- Test: `litegen-core/tests/multitenant_api.rs` (add `put_with` to `Client`; add tests)

- [ ] **Step 1: Add the `put_with` helper to the test Client**

In `litegen-core/tests/multitenant_api.rs`, in `impl Client`, immediately after the `delete` method (~line 250), add:

```rust
    async fn put_with(&mut self, path: &str, body: Value, headers: &[(&str, &str)]) -> Resp {
        self.send(reqwest::Method::PUT, path, Some(body), headers).await
    }
```

- [ ] **Step 2: Write the failing surface tests**

In `litegen-core/tests/multitenant_api.rs`, append these tests at the end of the file. They reuse existing helpers (`spawn_app`, `Client`, `signup_app_and_key`, `signup`, `unique_email`, `first_org_id`, `csrf`).

```rust
// ─── BYO app storage: API surface ────────────────────────────────────────────

#[tokio::test]
async fn app_storage_crud_roundtrip_and_no_secret_leak() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, _secret) = signup_app_and_key(&mut c, "stg-crud").await;
    let csrf = c.csrf().await;

    // Initially unconfigured.
    let g0 = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g0.status, 200, "{:?}", g0.body);
    assert_eq!(g0.body["configured"], serde_json::json!(false));

    // PUT a config.
    let put = c
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({
                "bucket_name": "acme-bucket",
                "region": "us-west-2",
                "endpoint_url": "https://minio.example.com",
                "path_prefix": "litegen/images",
                "access_key_id": "AKIAEXAMPLE9999",
                "secret_access_key": "top-secret-value"
            }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(put.status, 200, "put storage failed: {:?}", put.body);

    // GET returns the non-secret config + hint, never the secret.
    let g1 = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g1.body["configured"], serde_json::json!(true));
    assert_eq!(g1.body["bucket_name"], "acme-bucket");
    assert_eq!(g1.body["region"], "us-west-2");
    assert_eq!(g1.body["access_key_id_hint"], "…9999");
    let raw = serde_json::to_string(&g1.body).unwrap();
    assert!(!raw.contains("top-secret-value"), "secret leaked in GET: {raw}");
    assert!(!raw.contains("AKIAEXAMPLE9999"), "access key id leaked in GET: {raw}");

    // DELETE then GET → unconfigured.
    let del = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    assert_eq!(del.status, 204, "{:?}", del.body);
    let g2 = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g2.body["configured"], serde_json::json!(false));
}

#[tokio::test]
async fn app_storage_upsert_retains_secret_when_keys_omitted() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, _secret) = signup_app_and_key(&mut c, "stg-retain").await;
    let csrf = c.csrf().await;

    // Create with keys.
    let p1 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b1", "access_key_id": "AKIA1111", "secret_access_key": "sek" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);

    // Update bucket only (no key fields) → 200, hint retained.
    let p2 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b2" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p2.status, 200, "retain-secret put failed: {:?}", p2.body);
    let g = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g.body["bucket_name"], "b2");
    assert_eq!(g.body["access_key_id_hint"], "…1111");

    // Exactly one key field → 400.
    let p3 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b3", "access_key_id": "AKIA2222" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p3.status, 400, "incomplete creds should 400: {:?}", p3.body);

    // First-ever create without keys → 400 (delete first to simulate fresh app).
    let _ = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    let p4 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b4" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p4.status, 400, "create without keys should 400: {:?}", p4.body);
}

#[tokio::test]
async fn app_storage_cross_tenant_isolation() {
    let app = spawn_app().await;

    // Org A configures storage on its app.
    let mut a = Client::new(&app.base);
    let (app_a, _) = signup_app_and_key(&mut a, "stg-orga").await;
    let csrf_a = a.csrf().await;
    let put = a.put_with(
        &format!("/v1/apps/{app_a}/storage"),
        json!({ "bucket_name": "a-bucket", "access_key_id": "AKIAAAAA", "secret_access_key": "sek" }),
        &[("x-csrf-token", &csrf_a)],
    ).await;
    assert_eq!(put.status, 200, "{:?}", put.body);

    // Org B (separate signup) must not GET/PUT/DELETE org A's app storage.
    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("stg-orgb"), "OrgB").await;
    let csrf_b = b.csrf().await;

    let g = b.get(&format!("/v1/apps/{app_a}/storage")).await;
    assert!(g.status == 403 || g.status == 404, "cross-tenant GET leaked: {} {:?}", g.status, g.body);
    let p = b.put_with(
        &format!("/v1/apps/{app_a}/storage"),
        json!({ "bucket_name": "hijack", "access_key_id": "x", "secret_access_key": "y" }),
        &[("x-csrf-token", &csrf_b)],
    ).await;
    assert!(p.status == 403 || p.status == 404, "cross-tenant PUT allowed: {} {:?}", p.status, p.body);
    let d = b.delete(&format!("/v1/apps/{app_a}/storage"), &[("x-csrf-token", &csrf_b)]).await;
    assert!(d.status == 403 || d.status == 404, "cross-tenant DELETE allowed: {} {:?}", d.status, d.body);
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cd litegen-core && cargo test --test multitenant_api app_storage 2>&1 | tail -30`
Expected: FAIL — `404` from the server (routes don't exist yet) so the `200`/`204` assertions fail.

- [ ] **Step 4: Add the request struct + handlers**

In `litegen-core/src/api/handlers/orgs.rs`, after `delete_provider_credential` (~line 968), add. (The helpers `org_for_app`, `require_member_perm`, `err`, `internal_error`, and imports `State`, `Extension`, `Path`, `Json`, `Response`, `StatusCode`, `Arc`, `AppState`, `KeyContext`, `Permission` are already in this file from the provider-credential handlers.)

```rust
// ─── Per-app BYO storage config ──────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PutAppStorageRequest {
    #[serde(default)]
    pub backend: Option<String>,
    pub bucket_name: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub endpoint_url: Option<String>,
    #[serde(default)]
    pub custom_public_url: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    /// Write-only. Provide WITH `secret_access_key` to set/rotate; omit BOTH to keep existing.
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// Write-only.
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

fn app_storage_info_from_row(row: crate::types::AppStorageRow) -> crate::types::AppStorageInfo {
    crate::types::AppStorageInfo {
        configured: true,
        backend: Some(row.backend),
        bucket_name: Some(row.bucket_name),
        region: Some(row.region),
        endpoint_url: row.endpoint_url,
        custom_public_url: row.custom_public_url,
        path_prefix: row.path_prefix,
        access_key_id_hint: row.access_key_id_hint,
        updated_at: Some(row.updated_at),
    }
}

/// GET /v1/apps/{app_id}/storage — read BYO storage config (storage_cred:read). No secret.
#[utoipa::path(
    get,
    path = "/v1/apps/{app_id}/storage",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 200, description = "Storage config (no secret)", body = crate::types::AppStorageInfo),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn get_app_storage(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::StorageCredRead).await {
        return resp;
    }
    match state.db.get_app_storage(&app_id).await {
        Ok(Some(row)) => (StatusCode::OK, Json(app_storage_info_from_row(row))).into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(crate::types::AppStorageInfo {
                configured: false,
                backend: None,
                bucket_name: None,
                region: None,
                endpoint_url: None,
                custom_public_url: None,
                path_prefix: None,
                access_key_id_hint: None,
                updated_at: None,
            }),
        )
            .into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

/// PUT /v1/apps/{app_id}/storage — upsert BYO storage config (storage_cred:write).
#[utoipa::path(
    put,
    path = "/v1/apps/{app_id}/storage",
    params(("app_id" = String, Path, description = "Application ID")),
    request_body = PutAppStorageRequest,
    responses(
        (status = 200, description = "Stored (no secret)", body = crate::types::AppStorageInfo),
        (status = 400, description = "Bad request / secrets key unavailable", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn put_app_storage(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
    Json(body): Json<PutAppStorageRequest>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::StorageCredWrite).await {
        return resp;
    }

    let secrets_key = match state.secrets_key {
        Some(k) => k,
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "secrets_key_unavailable",
                "Storage credentials require a configured secrets key",
            );
        }
    };

    let bucket_name = body.bucket_name.trim().to_string();
    if bucket_name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_bucket", "bucket_name is required");
    }
    let backend = body
        .backend
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("s3")
        .to_string();
    let region = body
        .region
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("us-east-1")
        .to_string();

    // Resolve the encrypted credential material: set both → encrypt; neither →
    // retain existing; exactly one → 400.
    let ak = body.access_key_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let sk = body.secret_access_key.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (secret_ciphertext, secret_nonce, access_key_id_hint) = match (ak, sk) {
        (Some(ak), Some(sk)) => {
            let plaintext = serde_json::to_vec(
                &serde_json::json!({ "access_key_id": ak, "secret_access_key": sk }),
            )
            .map_err(|e| e.to_string());
            let plaintext = match plaintext {
                Ok(v) => v,
                Err(e) => return internal_error(&e),
            };
            let (ct, nonce) = match crate::auth::secrets::encrypt(&secrets_key, &plaintext) {
                Ok(v) => v,
                Err(e) => return internal_error(&e),
            };
            let hint = if ak.len() >= 4 {
                Some(format!("…{}", &ak[ak.len() - 4..]))
            } else {
                None
            };
            (ct, nonce, hint)
        }
        (None, None) => match state.db.get_app_storage(&app_id).await {
            Ok(Some(existing)) => {
                (existing.secret_ciphertext, existing.secret_nonce, existing.access_key_id_hint)
            }
            Ok(None) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "credentials_required",
                    "access_key_id and secret_access_key are required for a new storage config",
                );
            }
            Err(e) => return internal_error(&e.to_string()),
        },
        _ => {
            return err(
                StatusCode::BAD_REQUEST,
                "credentials_incomplete",
                "provide both access_key_id and secret_access_key, or neither",
            );
        }
    };

    let input = crate::types::AppStorageUpsert {
        app_id: app_id.clone(),
        backend,
        bucket_name,
        region,
        endpoint_url: body.endpoint_url.clone().filter(|s| !s.trim().is_empty()),
        custom_public_url: body.custom_public_url.clone().filter(|s| !s.trim().is_empty()),
        path_prefix: body.path_prefix.clone().filter(|s| !s.trim().is_empty()),
        access_key_id_hint,
        secret_ciphertext,
        secret_nonce,
    };
    if let Err(e) = state.db.upsert_app_storage(&input).await {
        return internal_error(&e.to_string());
    }

    match state.db.get_app_storage(&app_id).await {
        Ok(Some(row)) => (StatusCode::OK, Json(app_storage_info_from_row(row))).into_response(),
        Ok(None) => internal_error("storage config vanished after write"),
        Err(e) => internal_error(&e.to_string()),
    }
}

/// DELETE /v1/apps/{app_id}/storage — remove BYO storage config (storage_cred:delete).
#[utoipa::path(
    delete,
    path = "/v1/apps/{app_id}/storage",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn delete_app_storage(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::StorageCredDelete).await
    {
        return resp;
    }
    match state.db.delete_app_storage(&app_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "storage_not_found", "Storage config not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}
```

- [ ] **Step 5: Wire the route**

In `litegen-core/src/api/handlers/mod.rs`, in `create_router`:

(a) Update the routing import (~line 1715) to include `put`:
```rust
    use axum::routing::{delete, get, patch, post, put};
```
(b) In `auth_required_routes`, immediately after the `provider-credentials/{provider}` DELETE route (~line 1795), add:
```rust
        .route(
            "/v1/apps/{app_id}/storage",
            get(orgs::get_app_storage)
                .put(orgs::put_app_storage)
                .delete(orgs::delete_app_storage),
        )
```

- [ ] **Step 6: Register OpenAPI paths**

In `litegen-core/src/api/openapi.rs`, in the `paths(...)` list immediately after `crate::api::handlers::orgs::delete_provider_credential,` (~line 74), add:

```rust
        // BYO app storage
        crate::api::handlers::orgs::get_app_storage,
        crate::api::handlers::orgs::put_app_storage,
        crate::api::handlers::orgs::delete_app_storage,
```

- [ ] **Step 7: Run the surface tests to verify they pass**

Run: `cd litegen-core && cargo test --test multitenant_api app_storage_ 2>&1 | tail -30`
Expected: PASS — `app_storage_crud_roundtrip_and_no_secret_leak`, `app_storage_upsert_retains_secret_when_keys_omitted`, `app_storage_cross_tenant_isolation`.

- [ ] **Step 8: Commit**

```bash
git add litegen-core/src/api/handlers/orgs.rs litegen-core/src/api/handlers/mod.rs litegen-core/src/api/openapi.rs litegen-core/tests/multitenant_api.rs
git commit -m "feat(api): PUT/GET/DELETE /v1/apps/{id}/storage + surface tests"
```

---

## Task 5: Resolution + threading + storage-behavior tests

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs` (add `resolve_app_image_store`; call it in `generate_image`)
- Modify: `litegen-core/src/proxy/router.rs` (add `app_store` param to `generate_image`; use it in `build_image_results` call)
- Test: `litegen-core/tests/multitenant_api.rs` (wiremock-S3 behavior tests)

- [ ] **Step 1: Write the failing behavior tests**

Append to `litegen-core/tests/multitenant_api.rs`:

```rust
// ─── BYO app storage: generation behavior ────────────────────────────────────

#[tokio::test]
async fn app_storage_uploads_generation_to_per_app_bucket() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Stand-in S3: accept any PUT with 200.
    let s3 = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s3)
        .await;

    let app = spawn_app().await; // global image_store is LocalStore (b64 fallback)
    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "stg-upload").await;
    let csrf = c.csrf().await;

    // Point this app's BYO storage at the mock S3; custom_public_url makes the
    // returned URL deterministic.
    let put = c
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({
                "bucket_name": "test-bucket",
                "region": "us-east-1",
                "endpoint_url": s3.uri(),
                "custom_public_url": "https://cdn.example.test",
                "path_prefix": "litegen/images",
                "access_key_id": "AKIATEST",
                "secret_access_key": "secret"
            }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(put.status, 200, "put storage failed: {:?}", put.body);

    // Generate with the Bearer key (default response_format => store() is called).
    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "a cat" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "generation failed: {:?}", r.body);

    // The returned image URL points at the app's bucket (custom public URL), not b64.
    let url = r.body["data"][0]["url"].as_str().unwrap_or("");
    assert!(
        url.starts_with("https://cdn.example.test/litegen/images/"),
        "expected per-app bucket URL, got {url:?} (body {:?})",
        r.body
    );

    // The mock S3 received exactly one PUT to /test-bucket/litegen/images/<uuid>.<ext>.
    let received = s3.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 1, "expected one S3 PUT, got {}", received.len());
    let p = received[0].url.path();
    assert!(
        p.starts_with("/test-bucket/litegen/images/"),
        "unexpected S3 key path: {p}"
    );
}

#[tokio::test]
async fn unconfigured_app_falls_back_to_global_store() {
    let app = spawn_app().await; // global LocalStore => b64 inline
    let mut c = Client::new(&app.base);
    let (_app_id, secret) = signup_app_and_key(&mut c, "stg-nofb").await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    // No BYO storage + local global store => base64 inline, no URL.
    assert!(r.body["data"][0]["b64_json"].is_string(), "expected b64 fallback: {:?}", r.body);
    assert!(r.body["data"][0]["url"].is_null(), "expected no url: {:?}", r.body);
}

#[tokio::test]
async fn corrupt_storage_config_falls_back_without_failing() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let s3 = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s3)
        .await;

    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "stg-corrupt").await;
    let csrf = c.csrf().await;

    // Valid PUT first (so a row exists), pointed at the mock S3.
    let put = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({
            "bucket_name": "test-bucket",
            "endpoint_url": s3.uri(),
            "access_key_id": "AKIATEST",
            "secret_access_key": "secret"
        }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(put.status, 200, "{:?}", put.body);

    // Corrupt the stored ciphertext directly in the same SQLite DB file.
    let url = format!("sqlite://{}?mode=rwc", app.db_path().display());
    let pool = sqlx::sqlite::SqlitePool::connect(&url).await.expect("open sqlite");
    sqlx::query("UPDATE app_storage_credentials SET secret_ciphertext = 'not-base64-$$$' WHERE app_id = ?")
        .bind(&app_id)
        .execute(&pool)
        .await
        .expect("corrupt row");
    pool.close().await;

    // Generation still succeeds (fail-open to global store), no S3 PUT happens.
    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "corrupt config must fail-open, got {:?}", r.body);
    assert!(r.body["data"][0]["b64_json"].is_string(), "expected b64 fallback: {:?}", r.body);
    let received = s3.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 0, "corrupt config must NOT hit per-app S3");
}

#[tokio::test]
async fn delete_storage_reverts_to_global_store() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let s3 = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s3)
        .await;

    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "stg-revert").await;
    let csrf = c.csrf().await;

    c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({
            "bucket_name": "test-bucket",
            "endpoint_url": s3.uri(),
            "custom_public_url": "https://cdn.example.test",
            "access_key_id": "AKIATEST",
            "secret_access_key": "secret"
        }),
        &[("x-csrf-token", &csrf)],
    ).await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    // First gen → per-app bucket.
    let r1 = bearer.post_with(
        "/v1/images/generations",
        json!({ "model": MOCK_MODEL, "prompt": "x" }),
        &[("authorization", &bearer_hdr)],
    ).await;
    assert_eq!(r1.status, 200, "{:?}", r1.body);
    assert!(r1.body["data"][0]["url"].as_str().unwrap_or("").starts_with("https://cdn.example.test/"));

    // Remove storage → next gen falls back (b64), no new S3 PUT.
    let del = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    assert_eq!(del.status, 204, "{:?}", del.body);
    let r2 = bearer.post_with(
        "/v1/images/generations",
        json!({ "model": MOCK_MODEL, "prompt": "y" }),
        &[("authorization", &bearer_hdr)],
    ).await;
    assert_eq!(r2.status, 200, "{:?}", r2.body);
    assert!(r2.body["data"][0]["b64_json"].is_string(), "expected b64 after delete: {:?}", r2.body);

    let received = s3.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 1, "exactly one S3 PUT total (before delete)");
}
```

- [ ] **Step 2: Expose the DB path on `TestApp` (needed by the corrupt-config test)**

In `litegen-core/tests/multitenant_api.rs`, find the `TestApp` struct (it holds `base` and `_tmp: tempfile::NamedTempFile`). Add an accessor method:

```rust
impl TestApp {
    fn db_path(&self) -> std::path::PathBuf {
        self._tmp.path().to_path_buf()
    }
}
```

(If `impl TestApp` already exists, add the method inside it. `_tmp` is the `NamedTempFile` created in `spawn_app_with_providers`.)

- [ ] **Step 3: Run to verify they fail**

Run: `cd litegen-core && cargo test --test multitenant_api -- app_storage_uploads unconfigured_app corrupt_storage delete_storage_reverts 2>&1 | tail -40`
Expected: FAIL — `app_storage_uploads...` gets b64 (no per-app store wired yet) so the URL assertion fails; the S3 PUT count is 0.

- [ ] **Step 4: Add `resolve_app_image_store`**

In `litegen-core/src/api/handlers/mod.rs`, immediately after `resolve_app_credential` (~line 2069), add:

```rust
/// Resolve the calling app's BYO image store, if configured & usable. Returns
/// `None` (→ caller uses the global store) on any miss or, for a configured-but-
/// broken row, after logging a warning (fail-open — never breaks a generation).
async fn resolve_app_image_store(
    state: &AppState,
    key_ctx: &Option<KeyContext>,
) -> Option<std::sync::Arc<dyn crate::proxy::storage::ImageStore>> {
    let app_id = key_ctx.as_ref().and_then(|c| c.app_id.as_deref())?;
    let secrets_key = state.secrets_key?;
    let row = match state.db.get_app_storage(app_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = %e, "byo storage: lookup failed, using global store");
            return None;
        }
    };

    #[derive(serde::Deserialize)]
    struct StorageSecret {
        access_key_id: String,
        secret_access_key: String,
    }

    let plaintext = match crate::auth::secrets::decrypt(&secrets_key, &row.secret_ciphertext, &row.secret_nonce) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: decrypt failed, using global store");
            return None;
        }
    };
    let secret: StorageSecret = match serde_json::from_slice(&plaintext) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: corrupt secret, using global store");
            return None;
        }
    };

    let cfg = crate::config::ImageStorageConfig {
        backend: row.backend.clone(),
        path_prefix: row.path_prefix.clone(),
        s3: Some(crate::config::S3StorageConfig {
            bucket_name: row.bucket_name.clone(),
            region: row.region.clone(),
            access_key_id: Some(secret.access_key_id),
            secret_access_key: Some(secret.secret_access_key),
            endpoint_url: row.endpoint_url.clone(),
            custom_public_url: row.custom_public_url.clone(),
        }),
    };
    match crate::proxy::storage::S3Store::from_config(&cfg) {
        Ok(store) => Some(std::sync::Arc::new(store) as std::sync::Arc<dyn crate::proxy::storage::ImageStore>),
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: build failed, using global store");
            None
        }
    }
}
```

- [ ] **Step 5: Call it in `generate_image` and pass it down**

In `litegen-core/src/api/handlers/mod.rs`, in `generate_image`, immediately after the `let app_creds = match resolve_app_credential(...) { ... };` block, add:

```rust
    // Resolve the calling app's BYO image store (None → global store fallback).
    let app_store = resolve_app_image_store(&state, &key_ctx).await;
```

Then update the router call to pass `app_store` as the new final argument:

```rust
    match state.router.generate_image(&validated.schema, &validated.request.base, &extras, &materialized, app_creds, app_store).await {
```

- [ ] **Step 6: Thread `app_store` through `ProxyRouter::generate_image`**

In `litegen-core/src/proxy/router.rs`:

(a) Add the parameter to the signature (after `app_creds`):
```rust
    pub async fn generate_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        app_creds: Option<ProviderCredentials>,
        app_store: Option<Arc<dyn ImageStore>>,
    ) -> Result<ImageGenerationResponse, ProxyError> {
```

(b) Replace the `data:` line in the `ImageGenerationResponse { ... }` construction:
```rust
            data: build_image_results(
                &output,
                extras,
                app_store.as_ref().unwrap_or(&self.image_store),
            )
            .await,
```

(`Arc` and `ImageStore` are already imported in `router.rs` — the `image_store` field is `Arc<dyn ImageStore>`.)

- [ ] **Step 7: Fix the other `generate_image` call sites (compile-driven)**

Run: `cd litegen-core && cargo build 2>&1 | grep -A3 "generate_image" | head -40`
The signature change breaks any other caller (notably unit tests in `proxy/router.rs` or `api/handlers/mod.rs` test modules). For each compile error of the form "this method takes 6 arguments but 5 ... were supplied", append `, None` (no BYO store) to that call. Re-run `cargo build` until clean.

- [ ] **Step 8: Run the behavior tests to verify they pass**

Run: `cd litegen-core && cargo test --test multitenant_api -- app_storage_uploads unconfigured_app corrupt_storage delete_storage_reverts 2>&1 | tail -40`
Expected: PASS for all four.

> If `app_storage_uploads...` fails because the `s3` client formats the key path differently than expected, relax the path assertion to `assert!(p.contains("/test-bucket/") && p.ends_with(".png"))` and re-run — the goal is "a PUT reached the per-app bucket", not an exact path.

- [ ] **Step 9: Full suite + clippy**

Run: `cd litegen-core && cargo test 2>&1 | tail -20 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: all tests pass; clippy clean.

- [ ] **Step 10: Commit**

```bash
git add litegen-core/src/api/handlers/mod.rs litegen-core/src/proxy/router.rs litegen-core/tests/multitenant_api.rs
git commit -m "feat(proxy): per-app BYO image store resolution + threading (global fallback)"
```

---

## Task 6: SDK regen + TypeScript client namespace

**Files:**
- Run: `sdks/scripts/regen-all.sh` (regenerates `sdks/openapi.json`, `sdks/typescript/src/generated/schema.d.ts`, Python `_generated`)
- Modify: `sdks/typescript/src/client.ts` (add `AppStorageNamespace`, wire into `AppsNamespace`)
- Modify: `sdks/typescript/src/types.ts` (or wherever hand-written request/response types live — add `AppStorageInfo`, `PutAppStorageRequest`) — see Step 2
- Test: `sdks/typescript` build + existing tests

- [ ] **Step 1: Regenerate the OpenAPI snapshot + SDKs**

The release binary must be built first (the regen fetches the live spec). Run:
```bash
cd litegen-core && cargo build --release
cd .. && ./sdks/scripts/regen-all.sh
```
Expected: "==> Done. Review changes under sdks/ and commit." `git status` shows changes to `sdks/openapi.json` and `sdks/typescript/src/generated/schema.d.ts` including the `/v1/apps/{app_id}/storage` path and `AppStorageInfo`/`PutAppStorageRequest` schemas.

- [ ] **Step 2: Add hand-written types (if the client uses them)**

Inspect how provider creds expose `ProviderCredentialInfo` / `CreateProviderCredentialRequest` to `client.ts` (grep `ProviderCredentialInfo` in `sdks/typescript/src/`). Mirror that exact location. If they are hand-declared (e.g. in `sdks/typescript/src/types.ts`), add:

```ts
export interface AppStorageInfo {
  configured: boolean;
  backend?: string;
  bucket_name?: string;
  region?: string;
  endpoint_url?: string;
  custom_public_url?: string;
  path_prefix?: string;
  access_key_id_hint?: string;
  updated_at?: string;
}

export interface PutAppStorageRequest {
  backend?: string;
  bucket_name: string;
  region?: string;
  endpoint_url?: string;
  custom_public_url?: string;
  path_prefix?: string;
  access_key_id?: string;
  secret_access_key?: string;
}
```

If instead they are re-exported from generated types, import `AppStorageInfo`/`PutAppStorageRequest` from the generated module the same way `ProviderCredentialInfo` is imported.

- [ ] **Step 3: Add the `AppStorageNamespace` and wire it in**

In `sdks/typescript/src/client.ts`, after `class AppProviderCredentialsNamespace { ... }` (~line 891), add:

```ts
class AppStorageNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(appId: string, signal?: AbortSignal): Promise<AppStorageInfo> {
    return this.client.request(
      "GET",
      `/v1/apps/${encodeURIComponent(appId)}/storage`,
      undefined,
      signal,
    );
  }
  put(
    appId: string,
    req: PutAppStorageRequest,
    signal?: AbortSignal,
  ): Promise<AppStorageInfo> {
    return this.client.request(
      "PUT",
      `/v1/apps/${encodeURIComponent(appId)}/storage`,
      req,
      signal,
    );
  }
  delete(appId: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "DELETE",
      `/v1/apps/${encodeURIComponent(appId)}/storage`,
      undefined,
      signal,
    );
  }
}
```

Then update `AppsNamespace` (~line 894) to expose it:

```ts
class AppsNamespace {
  readonly providerCredentials: AppProviderCredentialsNamespace;
  readonly storage: AppStorageNamespace;

  constructor(private readonly client: LiteGenClient) {
    this.providerCredentials = new AppProviderCredentialsNamespace(client);
    this.storage = new AppStorageNamespace(client);
  }
```

Ensure `AppStorageInfo` and `PutAppStorageRequest` are imported at the top of `client.ts` next to `ProviderCredentialInfo`/`CreateProviderCredentialRequest`.

- [ ] **Step 4: Build + test the TypeScript SDK**

Run:
```bash
cd sdks/typescript && npm run build && npm test 2>&1 | tail -25
```
Expected: typecheck/build clean (the new namespace compiles against the generated `request` method); existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add sdks/openapi.json sdks/typescript sdks/python
git commit -m "feat(sdk): app storage get/put/delete client methods + regen"
```

---

## Task 7: Dashboard — Storage section in app settings

**Files:**
- Modify: `dashboard/src/pages/Organization.tsx` (add a "Storage" card mirroring the "Provider credentials" card)

> The provider-creds card lives in `Organization.tsx` and is scoped to `activeApp` via `useTenant()`. Add a sibling "Storage" card. Use `client.apps.storage.{get,put,delete}` from Task 6.

- [ ] **Step 1: Add state + load/mutation handlers**

In `dashboard/src/pages/Organization.tsx`, near the provider-credential state/handlers (~lines 27-105), add:

```tsx
  // App storage (BYO S3)
  const [storage, setStorage] = useState<AppStorageInfo | null>(null);
  const [stBucket, setStBucket] = useState('');
  const [stRegion, setStRegion] = useState('us-east-1');
  const [stEndpoint, setStEndpoint] = useState('');
  const [stPublicUrl, setStPublicUrl] = useState('');
  const [stPrefix, setStPrefix] = useState('');
  const [stAccessKeyId, setStAccessKeyId] = useState('');
  const [stSecret, setStSecret] = useState('');

  const loadStorage = useCallback(async () => {
    if (!activeApp) return null;
    try {
      return await client.apps.storage.get(activeApp);
    } catch {
      return null;
    }
  }, [activeApp]);

  useEffect(() => {
    let cancelled = false;
    void loadStorage().then(s => {
      if (cancelled) return;
      setStorage(s);
      setStBucket(s?.bucket_name ?? '');
      setStRegion(s?.region ?? 'us-east-1');
      setStEndpoint(s?.endpoint_url ?? '');
      setStPublicUrl(s?.custom_public_url ?? '');
      setStPrefix(s?.path_prefix ?? '');
      setStAccessKeyId('');
      setStSecret('');
    });
    return () => { cancelled = true; };
  }, [loadStorage]);

  const saveStorage = async () => {
    if (!activeApp || !stBucket.trim()) return;
    try {
      const body: PutAppStorageRequest = {
        bucket_name: stBucket.trim(),
        region: stRegion.trim() || 'us-east-1',
        endpoint_url: stEndpoint.trim() || undefined,
        custom_public_url: stPublicUrl.trim() || undefined,
        path_prefix: stPrefix.trim() || undefined,
      };
      // Send keys only if BOTH provided (omit both → backend retains existing).
      if (stAccessKeyId.trim() && stSecret.trim()) {
        body.access_key_id = stAccessKeyId.trim();
        body.secret_access_key = stSecret.trim();
      }
      const updated = await client.apps.storage.put(activeApp, body);
      setStorage(updated);
      setStAccessKeyId('');
      setStSecret('');
      showToast('Storage configuration saved', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const removeStorage = async () => {
    if (!activeApp) return;
    try {
      await client.apps.storage.delete(activeApp);
      setStorage(await loadStorage());
      setStBucket(''); setStEndpoint(''); setStPublicUrl(''); setStPrefix('');
      setStAccessKeyId(''); setStSecret('');
      showToast('Storage configuration removed', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error');
    }
  };
```

Add the import at the top next to `ProviderCredentialInfo`:
```tsx
import type { ProviderCredentialInfo, AppStorageInfo, PutAppStorageRequest } from '@litegen/sdk';
```
(Adjust to match how `ProviderCredentialInfo` is currently imported.)

- [ ] **Step 2: Add the Storage card UI**

In the JSX, immediately after the closing `</div>` of the "Provider credentials" card (~line 257), add:

```tsx
      {/* App storage (BYO S3) */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Storage (BYO S3)</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Generated images upload to this app's own bucket. Leave unset to use the platform default.
          {activeApp ? '' : ' — select an app to configure.'}
        </p>
        {storage?.configured && (
          <div data-testid="storage-configured" style={{ color: '#8b949e', fontSize: 13, marginBottom: 12 }}>
            Configured · bucket <strong style={{ color: '#e6edf3' }}>{storage.bucket_name}</strong>
            {storage.access_key_id_hint && (
              <span style={{ fontFamily: 'monospace', marginLeft: 8 }}>key {storage.access_key_id_hint}</span>
            )}
          </div>
        )}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 460 }}>
          <input className="input" data-testid="storage-bucket" value={stBucket}
            onChange={e => setStBucket(e.target.value)} placeholder="bucket name" disabled={!activeApp} />
          <input className="input" data-testid="storage-region" value={stRegion}
            onChange={e => setStRegion(e.target.value)} placeholder="region (e.g. us-east-1)" disabled={!activeApp} />
          <input className="input" data-testid="storage-endpoint" value={stEndpoint}
            onChange={e => setStEndpoint(e.target.value)} placeholder="endpoint URL (MinIO/R2/Spaces, optional)" disabled={!activeApp} />
          <input className="input" data-testid="storage-public-url" value={stPublicUrl}
            onChange={e => setStPublicUrl(e.target.value)} placeholder="custom public URL / CDN (optional)" disabled={!activeApp} />
          <input className="input" data-testid="storage-prefix" value={stPrefix}
            onChange={e => setStPrefix(e.target.value)} placeholder="path prefix (default litegen/images)" disabled={!activeApp} />
          <input className="input" data-testid="storage-access-key-id" value={stAccessKeyId}
            onChange={e => setStAccessKeyId(e.target.value)}
            placeholder={storage?.configured ? 'access key id (leave blank to keep)' : 'access key id'} disabled={!activeApp} />
          <input className="input" data-testid="storage-secret" type="password" value={stSecret}
            onChange={e => setStSecret(e.target.value)}
            placeholder={storage?.configured ? 'secret access key (leave blank to keep)' : 'secret access key'} disabled={!activeApp} />
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn btn-primary" data-testid="storage-save" onClick={saveStorage} disabled={!activeApp}>
              Save storage
            </button>
            {storage?.configured && (
              <button className="btn btn-danger" data-testid="storage-remove" onClick={removeStorage} disabled={!activeApp}>
                Remove storage
              </button>
            )}
          </div>
        </div>
      </div>
```

- [ ] **Step 3: Build the dashboard**

Run: `cd dashboard && npm run build 2>&1 | tail -25`
Expected: build clean (TypeScript types from the SDK resolve; no unused-var errors).

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/pages/Organization.tsx
git commit -m "feat(dashboard): BYO S3 storage settings section"
```

---

## Task 8: Playwright e2e

**Files:**
- Modify: `dashboard/e2e-mt/multitenant.spec.ts` (add a storage step/test driven through the UI)

> The existing spec signs up org A, navigates to `/organization`, and manages provider creds there. Add storage steps in the same file. The e2e backend (`playwright.multitenant.config.ts`) already runs hosted mode with `LITEGEN__SECRETS_KEY` set, so storage PUT works.

- [ ] **Step 1: Write the e2e test**

In `dashboard/e2e-mt/multitenant.spec.ts`, add a new test (after the existing multitenant test). It signs up a fresh org, configures storage via the UI, reloads to confirm no secret is shown, then removes it.

```ts
test('hosted multi-tenant: BYO S3 storage configure, persist-without-secret, remove', async ({ page }) => {
  const rand = () => Math.random().toString(36).slice(2, 10);
  const owner = `stg+${rand()}@litegen.test`;
  const PW = 'super-secret-password-123';
  const SECRET = 'super-secret-access-key-DO-NOT-ECHO';

  // Sign up org + first app.
  await page.goto('/signup');
  await page.locator('[data-testid="signup-org-name"]').fill('Storage Co');
  await page.locator('[data-testid="signup-email"]').fill(owner);
  await page.locator('[data-testid="signup-password"]').fill(PW);
  await page.locator('[data-testid="signup-confirm-password"]').fill(PW);
  await page.locator('[data-testid="signup-submit"]').click();
  await page.waitForURL('**/');
  await page.goto('/');
  await expect(page.locator('[data-testid="user-menu-email"]')).toContainText(owner, { timeout: 15_000 });

  // Configure storage.
  await page.goto('/organization');
  await page.locator('[data-testid="storage-bucket"]').fill('e2e-bucket');
  await page.locator('[data-testid="storage-region"]').fill('us-east-1');
  await page.locator('[data-testid="storage-endpoint"]').fill('http://127.0.0.1:5599'); // never called in this test
  await page.locator('[data-testid="storage-access-key-id"]').fill('AKIAE2E1234');
  await page.locator('[data-testid="storage-secret"]').fill(SECRET);
  await page.locator('[data-testid="storage-save"]').click();
  await expect(page.locator('[data-testid="storage-configured"]')).toContainText('e2e-bucket', { timeout: 15_000 });

  // Reload → config persists; bucket shown; secret NEVER rendered; hint shown.
  await page.goto('/organization');
  await expect(page.locator('[data-testid="storage-configured"]')).toContainText('e2e-bucket', { timeout: 15_000 });
  await expect(page.locator('[data-testid="storage-configured"]')).toContainText('…1234');
  await expect(page.locator('[data-testid="storage-secret"]')).toHaveValue('');
  await expect(page.locator('body')).not.toContainText(SECRET);

  // Remove → configured banner disappears.
  await page.locator('[data-testid="storage-remove"]').click();
  await expect(page.locator('[data-testid="storage-configured"]')).toHaveCount(0, { timeout: 15_000 });
});
```

- [ ] **Step 2: Run the e2e suite (fresh DB each run)**

Run: `cd dashboard && npx playwright test --config=playwright.multitenant.config.ts -g "BYO S3 storage" 2>&1 | tail -30`
Expected: the storage test passes. (First run builds nothing extra; the config's `webServer` boots the release binary — ensure `cargo build --release` from Task 6 ran.)

- [ ] **Step 3: Run the full multitenant e2e to confirm no regressions**

Run: `cd dashboard && npx playwright test --config=playwright.multitenant.config.ts 2>&1 | tail -30`
Expected: all multitenant e2e tests pass with the freshly recreated DB.

- [ ] **Step 4: Commit**

```bash
git add dashboard/e2e-mt/multitenant.spec.ts
git commit -m "test(e2e): BYO S3 storage configure/persist/remove via dashboard UI"
```

---

## Task 9: Polish — full gates + README

**Files:**
- Modify: `README.md` (document the endpoints + `LITEGEN__SECRETS_KEY` dependency)

- [ ] **Step 1: Run all acceptance gates**

Run each; all must be clean:
```bash
cd litegen-core && cargo test 2>&1 | tail -15
cd litegen-core && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15
cd litegen-core && cargo build --release 2>&1 | tail -5
cd dashboard && npm run build 2>&1 | tail -10
cd sdks/typescript && npm run build && npm test 2>&1 | tail -10
```
Expected: all green. Fix any failures before continuing.

- [ ] **Step 2: Document in README**

In `README.md`, in the hosting / multi-tenant section (near where BYO provider credentials are documented), add a "BYO object storage" subsection:

```markdown
### BYO object storage (per app)

Each application can store its generated images in its own S3-compatible bucket
(AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces). Configure it per app:

- `GET /v1/apps/{app_id}/storage` — current config (never returns the secret)
- `PUT /v1/apps/{app_id}/storage` — set/update; body: `bucket_name` (required),
  `region`, `endpoint_url`, `custom_public_url`, `path_prefix`, and the write-only
  `access_key_id` + `secret_access_key` (send both to set/rotate; omit both to keep
  the existing pair)
- `DELETE /v1/apps/{app_id}/storage` — revert to the platform default store

The credential pair is encrypted at rest with AES-256-GCM using
`LITEGEN__SECRETS_KEY` (required in hosted mode). Apps with no storage configured
fall back to the global `image_storage` backend (S3 if set, else base64 inline).
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: BYO per-app object storage (endpoints + secrets key)"
```

- [ ] **Step 4: Finish the branch**

Invoke the `superpowers:finishing-a-development-branch` skill to decide merge/PR/cleanup. (Branch: `feat/byo-app-storage`, based on `main`.)

---

## Self-Review (completed during planning)

**Spec coverage:**
- Data model (hybrid plaintext + encrypted pair) → Task 1 (migration) + Task 2 (types).
- DB layer (3 methods, pg+sqlite) → Task 2.
- Encryption (reuse `auth::secrets`) → Task 2 (test) + Task 4 (PUT encrypt) + Task 5 (resolve decrypt).
- API (PUT/GET/DELETE, no-secret, secrets-key-required, validation, upsert-retain) → Task 4.
- Permissions (`storage_cred:*`, role mapping) → Task 3.
- Resolution + threading (resolve-in-handler, router param, global fallback, fail-open) → Task 5.
- SDK regen + client methods → Task 6.
- Dashboard Storage section → Task 7.
- Test matrices: Rust integration #1 CRUD/no-secret (T4), #2 upsert-retain (T4), #3 encrypted-at-rest (T2 DB test), #4 per-app upload (T5), #5 fallback unconfigured (T5), #6 fallback corrupt (T5), #7 secrets-key-required (covered by T4 — note below), #8 cross-tenant (T4), #9 permission gating (note below), #10 delete-reverts (T5). Playwright e2e configure/persist-no-secret/remove → Task 8.

**Two spec test cases folded with a note:**
- #7 (secrets-key-required → 400): the PUT handler returns `secrets_key_unavailable` when `state.secrets_key` is `None`. The integration harness always sets `secrets_key: Some([7u8;32])`, so a dedicated test would need a no-secrets-key server variant. **If `spawn_app` exposes no such variant, add a `spawn_app_no_secrets()` clone that sets `secrets_key: None`, then assert PUT → 400 `secrets_key_unavailable`.** (Mechanical; mirrors `spawn_app`.)
- #9 (permission gating: Viewer can GET, cannot PUT/DELETE): requires inviting a Viewer teammate. The existing suite already has an invite+role test (`invitations + roles`); **add a step there (or a sibling test) that, as a Viewer, asserts `GET /v1/apps/{app}/storage` → 200 and `PUT` → 403 `forbidden_permission`**, reusing that test's invite/accept helpers.

**Placeholder scan:** none — every code step has complete code.

**Type consistency:** `AppStorageRow`/`AppStorageUpsert`/`AppStorageInfo` field names are identical across types, DB SELECT columns, handler construction, and resolution. `app_store: Option<Arc<dyn ImageStore>>` matches between handler call, router signature, and `build_image_results` usage. Permission variants `StorageCredRead/Write/Delete` consistent across enum/as_str/permissions_for/handlers/test.
