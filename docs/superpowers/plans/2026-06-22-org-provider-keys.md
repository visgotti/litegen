# Org-Scoped Provider Keys + Provider-Centric Providers Page — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move provider API keys from app scope to org scope (every app in an org reuses one key), and rebuild the Providers page into provider-centric sections with a model feature-detail drawer.

**Architecture:** Phase 1 (backend) adds an `org_provider_credentials` table + org-keyed DB methods + `/v1/orgs/{id}/provider-credentials` routes, switches request-time credential resolution to read `KeyContext.org_id` / `Generation.org_id`, and removes the per-app credential surface. Phase 2 (frontend) rebuilds `Providers.tsx` into collapsible per-provider sections (org key + auto-saving model toggles) and adds a shared `ModelDetail` drawer reused by `/models`.

**Tech Stack:** Rust (axum, sqlx — sqlite + postgres), TypeScript SDK, React (Vite) dashboard, Playwright e2e.

## Global Constraints

- Provider keys are **org-scoped only** — no per-app key override.
- Per-app layer stays a granular allowlist (`app_model_access`, `all` | `select`); it reuses the org key.
- Migrations are **dual-file**: every schema change lands in BOTH `litegen-core/migrations/sqlite/` and `litegen-core/migrations/postgres/` with the same `2024010100001N_*.sql` name.
- sqlx decodes Postgres strictly by type (NUMERIC↛f64 etc.); gate PG-specific regression tests behind `LITEGEN_PG_TEST_URL` in `litegen-core/src/db/postgres_tests.rs` (SQLite tests will not catch type mismatches).
- No `Co-Authored-By: Claude` / Claude attribution in commits.
- Permissions: list = `Permission::ProviderCredRead`, upsert = `Permission::ProviderCredWrite`, delete = `Permission::ProviderCredDelete` (unchanged enum; just re-scoped to org).
- Encrypted blob shape is unchanged: `serde_json` of `CreateProviderCredentialRequest.credentials`, AES via `crate::auth::secrets::{encrypt,decrypt}` with `state.secrets_key`.

---

# Phase 1 — Backend: org-scoped provider credentials

### Task 1: Migration — `org_provider_credentials` table + copy-up + drop app table

**Files:**
- Create: `litegen-core/migrations/sqlite/20240101000013_org_provider_credentials.sql`
- Create: `litegen-core/migrations/postgres/20240101000013_org_provider_credentials.sql`
- Test: `litegen-core/src/db/sqlite.rs` (add `#[cfg(test)]` test `migration_creates_org_cred_table_and_drops_app_table`)

**Interfaces:**
- Produces: table `org_provider_credentials(id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at)`, `UNIQUE(org_id, provider)`. Table `provider_credentials` no longer exists after migration.

- [ ] **Step 1: Write the failing test** (in `litegen-core/src/db/sqlite.rs`, inside the existing `#[cfg(test)] mod tests`. The DB type is `SqliteDatabase`; construct via `SqliteDatabase::connect("sqlite::memory:")` and read the pool via the existing `pub(crate) fn pool(&self) -> &SqlitePool` accessor — both already used by tests in this file.)

> Copy-up note: the data copy-up CANNOT be asserted after a full-migration run, because by the time migrations finish, migration 013 has already dropped `provider_credentials`. This automated test therefore asserts the *schema* outcome (new table present, old table gone). Validate the copy-up itself before deploy: on a *copy* of the live DB, temporarily comment out the `DROP TABLE` line, run the binary so migration 013 applies, and confirm each `(org, provider)` got the earliest app key in `org_provider_credentials`. Then restore the `DROP`.

```rust
#[tokio::test]
async fn migration_creates_org_cred_table_and_drops_app_table() {
    // Fresh in-memory DB with all migrations (incl. 013) applied.
    let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let pool = db.pool();

    // New org-scoped table exists and starts empty.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM org_provider_credentials").fetch_one(pool).await.unwrap();
    assert_eq!(n, 0, "fresh org cred table starts empty");

    // Old per-app table is gone.
    let dropped = sqlx::query("SELECT 1 FROM provider_credentials LIMIT 1").fetch_optional(pool).await;
    assert!(dropped.is_err(), "provider_credentials table should be dropped by migration 013");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p litegen --lib db::sqlite::tests::migration_creates_org_cred_table_and_drops_app_table`
Expected: FAIL — `org_provider_credentials` does not exist (migration not written yet).

- [ ] **Step 3: Write the sqlite migration**

`litegen-core/migrations/sqlite/20240101000013_org_provider_credentials.sql`:

```sql
-- Org-scoped provider credentials (replaces per-app provider_credentials).
CREATE TABLE org_provider_credentials (
    id            TEXT PRIMARY KEY,
    org_id        TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL,
    ciphertext    TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    display_hint  TEXT,
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (org_id, provider)
);

-- Best-effort copy-up: for each (org, provider) take the earliest-created app
-- credential. SQLite's bare-column rule returns the row matching MIN(created_at).
INSERT INTO org_provider_credentials (id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at)
SELECT pc.id, a.org_id, pc.provider, pc.ciphertext, pc.nonce, pc.display_hint, MIN(pc.created_at), pc.updated_at
FROM provider_credentials pc
JOIN applications a ON a.id = pc.app_id
GROUP BY a.org_id, pc.provider;

DROP TABLE provider_credentials;
```

- [ ] **Step 4: Write the postgres migration**

`litegen-core/migrations/postgres/20240101000013_org_provider_credentials.sql`:

```sql
CREATE TABLE org_provider_credentials (
    id            TEXT PRIMARY KEY,
    org_id        TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL,
    ciphertext    TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    display_hint  TEXT,
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (org_id, provider)
);

-- Postgres requires DISTINCT ON for first-writer-wins (no bare-column rule).
INSERT INTO org_provider_credentials (id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at)
SELECT DISTINCT ON (a.org_id, pc.provider)
       pc.id, a.org_id, pc.provider, pc.ciphertext, pc.nonce, pc.display_hint, pc.created_at, pc.updated_at
FROM provider_credentials pc
JOIN applications a ON a.id = pc.app_id
ORDER BY a.org_id, pc.provider, pc.created_at ASC;

DROP TABLE provider_credentials;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p litegen --lib db::sqlite::tests::migration_creates_org_cred_table_and_drops_app_table`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add litegen-core/migrations/sqlite/20240101000013_org_provider_credentials.sql \
        litegen-core/migrations/postgres/20240101000013_org_provider_credentials.sql \
        litegen-core/src/db/sqlite.rs
git commit -m "feat(db): org_provider_credentials table + copy-up migration"
```

---

### Task 2: DB trait + sqlite/postgres impls (org-keyed credential methods)

**Files:**
- Modify: `litegen-core/src/db/trait_def.rs:502-535` (replace the four app-keyed default methods)
- Modify: `litegen-core/src/db/sqlite.rs:1476-1545` (the four impls)
- Modify: `litegen-core/src/db/postgres.rs` (the four impls — find by `fn upsert_provider_credential`)
- Test: `litegen-core/src/db/sqlite.rs` test `org_provider_credential_roundtrip`

**Interfaces:**
- Produces (on `trait DatabaseStore`):
  - `async fn upsert_org_provider_credential(&self, org_id: &str, provider: &str, ciphertext: &str, nonce: &str, display_hint: Option<&str>) -> Result<(), sqlx::Error>`
  - `async fn get_org_provider_credential(&self, org_id: &str, provider: &str) -> Result<Option<(String, String)>, sqlx::Error>`
  - `async fn list_org_provider_credentials(&self, org_id: &str) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error>`
  - `async fn delete_org_provider_credential(&self, org_id: &str, provider: &str) -> Result<bool, sqlx::Error>`
- Removes: the four `*_provider_credential(app_id, …)` methods.

- [ ] **Step 1: Write the failing test** (`litegen-core/src/db/sqlite.rs` tests)

```rust
#[tokio::test]
async fn org_provider_credential_roundtrip() {
    let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    // org FK target
    sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES ('o1','Org1',datetime('now'))")
        .execute(db.pool()).await.unwrap();

    db.upsert_org_provider_credential("o1", "openai", "CT", "NONCE", Some("…1234")).await.unwrap();
    assert_eq!(db.get_org_provider_credential("o1", "openai").await.unwrap(), Some(("CT".into(), "NONCE".into())));

    let list = db.list_org_provider_credentials("o1").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].provider, "openai");
    assert_eq!(list[0].display_hint.as_deref(), Some("…1234"));

    // upsert overwrites
    db.upsert_org_provider_credential("o1", "openai", "CT2", "N2", Some("…9999")).await.unwrap();
    assert_eq!(db.get_org_provider_credential("o1", "openai").await.unwrap(), Some(("CT2".into(), "N2".into())));

    assert!(db.delete_org_provider_credential("o1", "openai").await.unwrap());
    assert_eq!(db.get_org_provider_credential("o1", "openai").await.unwrap(), None);
    assert!(!db.delete_org_provider_credential("o1", "openai").await.unwrap());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p litegen --lib db::sqlite::tests::org_provider_credential_roundtrip`
Expected: FAIL — methods not defined.

- [ ] **Step 3: Replace the trait defaults** in `litegen-core/src/db/trait_def.rs` (the block currently at lines 502-535):

```rust
    // ─── Provider Credentials (org-scoped; store opaque ciphertext only) ────

    async fn upsert_org_provider_credential(
        &self,
        _org_id: &str,
        _provider: &str,
        _ciphertext: &str,
        _nonce: &str,
        _display_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Returns `(ciphertext, nonce)` for the stored credential, if any.
    async fn get_org_provider_credential(
        &self,
        _org_id: &str,
        _provider: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        Ok(None)
    }

    async fn list_org_provider_credentials(
        &self,
        _org_id: &str,
    ) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> {
        Ok(vec![])
    }

    async fn delete_org_provider_credential(
        &self,
        _org_id: &str,
        _provider: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(false)
    }
```

- [ ] **Step 4: Replace the sqlite impls** (`litegen-core/src/db/sqlite.rs:1476-1545`). Same bodies as the app versions, but table `org_provider_credentials`, column `org_id`, conflict target `(org_id, provider)`:

```rust
    async fn upsert_org_provider_credential(
        &self, org_id: &str, provider: &str, ciphertext: &str, nonce: &str, display_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO org_provider_credentials \
                (id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT (org_id, provider) DO UPDATE SET \
                ciphertext = excluded.ciphertext, nonce = excluded.nonce, \
                display_hint = excluded.display_hint, updated_at = datetime('now')",
        )
        .bind(id.to_string()).bind(org_id).bind(provider).bind(ciphertext).bind(nonce).bind(display_hint)
        .execute(&self.pool).await?;
        Ok(())
    }

    async fn get_org_provider_credential(
        &self, org_id: &str, provider: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT ciphertext, nonce FROM org_provider_credentials WHERE org_id = ? AND provider = ?",
        ).bind(org_id).bind(provider).fetch_optional(&self.pool).await?;
        Ok(row)
    }

    async fn list_org_provider_credentials(
        &self, org_id: &str,
    ) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, Option<String>, chrono::DateTime<chrono::Utc>)>(
            "SELECT provider, display_hint, created_at FROM org_provider_credentials WHERE org_id = ? ORDER BY provider",
        ).bind(org_id).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(provider, display_hint, created_at)| ProviderCredentialInfo {
            provider, display_hint, created_at,
        }).collect())
    }

    async fn delete_org_provider_credential(
        &self, org_id: &str, provider: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM org_provider_credentials WHERE org_id = ? AND provider = ?")
            .bind(org_id).bind(provider).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }
```

> NOTE: copy the exact column-binding/`query_as` shapes from the existing app impls at `sqlite.rs:1476-1545` (e.g. the `created_at` decode type). If the app version mapped `created_at` differently, mirror that exactly.

- [ ] **Step 5: Replace the postgres impls** in `litegen-core/src/db/postgres.rs` — same four methods, but use `$1,$2,…` placeholders and `NOW()` instead of `datetime('now')`, mirroring the existing app postgres impls (find via `grep -n "fn upsert_provider_credential" litegen-core/src/db/postgres.rs`). Conflict target `(org_id, provider)`.

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p litegen --lib db::sqlite::tests::org_provider_credential_roundtrip`
Expected: PASS.

- [ ] **Step 7: Compile-check the whole crate** (the old methods are now gone; callers in Tasks 3-4 will be updated next — expect errors ONLY in `api/handlers/orgs.rs`, `api/handlers/mod.rs`, `proxy/poller.rs`)

Run: `cargo build -p litegen 2>&1 | grep -E "provider_credential" | head`
Expected: errors only in those three files (fixed in Tasks 3-4).

- [ ] **Step 8: Commit**

```bash
git add litegen-core/src/db/trait_def.rs litegen-core/src/db/sqlite.rs litegen-core/src/db/postgres.rs
git commit -m "feat(db): org-scoped provider credential trait + impls"
```

---

### Task 3: Org credential API routes + remove app routes

**Files:**
- Modify: `litegen-core/src/api/handlers/orgs.rs` (rewrite the three handlers to org scope; ~lines 834-1006)
- Modify: `litegen-core/src/api/handlers/mod.rs:2067-2073` (router) + remove from `openapi.rs` if listed
- Test: `litegen-core/src/api/handlers/orgs.rs` `#[cfg(test)] mod tests` (add `org_provider_credential_endpoints`)

**Interfaces:**
- Consumes: Task 2's `*_org_provider_credential` DB methods.
- Produces routes:
  - `GET    /v1/orgs/{org_id}/provider-credentials` → `list_org_provider_credentials`
  - `POST   /v1/orgs/{org_id}/provider-credentials` → `create_org_provider_credential`
  - `DELETE /v1/orgs/{org_id}/provider-credentials/{provider}` → `delete_org_provider_credential`

- [ ] **Step 1: Write the failing test** in `litegen-core/src/api/handlers/orgs/tests.rs`. Mirror the existing `provider_credential_store_and_list_no_plaintext` test exactly — same helpers (`build_db`, `seed_user`, `build_state(db, Some([7u8;32]))`, `create_router`, `cookie(&sess)`, `x-csrf-token`, `oneshot`, `body_json`) — but hit the org route and get `org_id` from the create-org response (no app needed):

```rust
#[tokio::test]
async fn org_provider_credential_store_list_delete() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), Some([7u8; 32])).await);

    // Create org → org_id.
    let req = Request::builder().method("POST").uri("/v1/orgs")
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap())).unwrap();
    let org_id = body_json(app.clone().oneshot(req).await.unwrap()).await["id"].as_str().unwrap().to_string();

    // POST a credential → masked hint, no plaintext.
    let req = Request::builder().method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "provider": "openai", "credentials": { "api_key": "sk-secret1234" } })).unwrap())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let info = body_json(resp).await;
    assert_eq!(info["provider"], "openai");
    assert_eq!(info["display_hint"], "…1234");
    assert!(!info.to_string().contains("sk-secret1234"), "plaintext must not leak");

    // GET list.
    let req = Request::builder().uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&sess)).body(Body::empty()).unwrap();
    let list = body_json(app.clone().oneshot(req).await.unwrap()).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // DELETE → 204, then empty.
    let req = Request::builder().method("DELETE")
        .uri(format!("/v1/orgs/{}/provider-credentials/openai", org_id))
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf).body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p litegen --lib api::handlers::orgs::tests::org_provider_credential_store_list_delete`
Expected: FAIL (compile error — handlers not defined yet).

- [ ] **Step 3: Rewrite the three handlers** in `orgs.rs`. They become a direct simplification of the app versions: the `org_id` comes straight from the path, so drop the `org_for_app(app_id)` call. Keep the existing `derive_display_hint`, encryption, and `CreateProviderCredentialRequest` body.

```rust
// ─── GET /v1/orgs/{org_id}/provider-credentials ─────────────────────────────
pub async fn list_org_provider_credentials(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(org_id): Path<String>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::ProviderCredRead).await {
        return resp;
    }
    match state.db.list_org_provider_credentials(&org_id).await {
        Ok(creds) => (StatusCode::OK, Json(creds)).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── POST /v1/orgs/{org_id}/provider-credentials ────────────────────────────
pub async fn create_org_provider_credential(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(org_id): Path<String>,
    Json(body): Json<CreateProviderCredentialRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::ProviderCredWrite).await {
        return resp;
    }
    let secrets_key = match state.secrets_key {
        Some(k) => k,
        None => return err(StatusCode::BAD_REQUEST, "secrets_not_configured",
            "Provider credentials require a configured secrets key"),
    };
    let provider = body.provider.trim().to_string();
    if provider.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_provider", "provider is required");
    }
    let display_hint = derive_display_hint(&body.credentials);
    let plaintext = match serde_json::to_vec(&body.credentials) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, "invalid_credentials", &e.to_string()),
    };
    let (ciphertext, nonce) = match crate::auth::secrets::encrypt(&secrets_key, &plaintext) {
        Ok(v) => v,
        Err(e) => return internal_error(&e),
    };
    if let Err(e) = state.db
        .upsert_org_provider_credential(&org_id, &provider, &ciphertext, &nonce, display_hint.as_deref())
        .await
    {
        return internal_error(&e.to_string());
    }
    let info = ProviderCredentialInfo { provider, display_hint, created_at: chrono::Utc::now() };
    (StatusCode::OK, Json(info)).into_response()
}

// ─── DELETE /v1/orgs/{org_id}/provider-credentials/{provider} ───────────────
pub async fn delete_org_provider_credential(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path((org_id, provider)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::ProviderCredDelete).await {
        return resp;
    }
    match state.db.delete_org_provider_credential(&org_id, &provider).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "credential_not_found", "Credential not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}
```

Delete the old `list_provider_credentials`, `create_provider_credential`, `delete_provider_credential` functions and their `#[utoipa::path]` blocks.

- [ ] **Step 4: Update the router** in `litegen-core/src/api/handlers/mod.rs`. Replace the app routes (currently ~2067-2073):

```rust
        .route(
            "/v1/orgs/{org_id}/provider-credentials",
            get(orgs::list_org_provider_credentials).post(orgs::create_org_provider_credential),
        )
        .route(
            "/v1/orgs/{org_id}/provider-credentials/{provider}",
            delete(orgs::delete_org_provider_credential),
        )
```

If `openapi.rs` references the old handler paths, update those entries to the new `orgs::*_org_provider_credential` symbols (and remove the `Applications`-tag cred paths).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p litegen --lib api::handlers::orgs::tests::org_provider_credential_store_list_delete`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/api/handlers/orgs.rs litegen-core/src/api/handlers/mod.rs litegen-core/src/api/openapi.rs
git commit -m "feat(api): org provider-credential routes; remove per-app routes"
```

---

### Task 4: Switch request-time credential resolution to org

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs:2496-2504` (sync request path)
- Modify: `litegen-core/src/proxy/poller.rs:186-189` (async polling path)
- Test: `litegen-core/src/api/handlers/mod.rs` or an integration test asserting a generation uses the org key.

**Interfaces:**
- Consumes: `KeyContext.org_id: Option<String>`, `Generation.org_id: Option<String>`, Task 2's `get_org_provider_credential`.

- [ ] **Step 1: Write the failing test** — extend the existing multitenant integration test (or add one) so a request with a configured ORG credential resolves it. If the crate has a credential-resolution unit test for the app path, port it to org. Minimal unit-level assertion:

```rust
#[tokio::test]
async fn resolves_org_credential_for_request() {
    let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES ('o1','O',datetime('now'))")
        .execute(db.pool()).await.unwrap();
    db.upsert_org_provider_credential("o1", "mock", "CT", "N", Some("…1")).await.unwrap();
    // get_org_provider_credential is what the resolver now calls:
    assert!(db.get_org_provider_credential("o1", "mock").await.unwrap().is_some());
    assert!(db.get_org_provider_credential("o2", "mock").await.unwrap().is_none());
}
```

- [ ] **Step 2: Run test to verify it fails/compiles**

Run: `cargo test -p litegen --lib resolves_org_credential_for_request`
Expected: PASS only after the resolver edits compile (the crate won't build until Step 3-4 fix the old `get_provider_credential` callers). So first expect a BUILD failure referencing `get_provider_credential`.

- [ ] **Step 3: Edit the sync resolver** in `mod.rs` (~2496-2504). Replace `app_id` extraction + `get_provider_credential(app_id, …)`:

```rust
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return Ok(None),
    };
    let secrets_key = match state.secrets_key {
        Some(k) => k,
        None => return Ok(None),
    };
    match state.db.get_org_provider_credential(org_id, provider).await {
        // ... rest of the match arms unchanged (decrypt → from_json) ...
```

- [ ] **Step 4: Edit the poller resolver** in `proxy/poller.rs` (`resolve_gen_credential`, ~186-189):

```rust
    let org_id = gen.org_id.as_deref()?;
    let key = secrets_key?;
    match db.get_org_provider_credential(org_id, &gen.provider).await {
        // ... unchanged decrypt/from_json arms ...
```

(Delete the now-unused `let app_id = gen.app_id.as_deref()?;` line.)

- [ ] **Step 5: Build + run test**

Run: `cargo build -p litegen && cargo test -p litegen --lib resolves_org_credential_for_request`
Expected: crate builds clean; test PASSES.

- [ ] **Step 6: Full backend test sweep**

Run: `cargo test -p litegen`
Expected: PASS (no remaining references to the removed app-cred methods/routes).

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/api/handlers/mod.rs litegen-core/src/proxy/poller.rs
git commit -m "feat(proxy): resolve provider credentials by org at request time"
```

---

### Task 5: SDK — org `providerCredentials` namespace; remove app namespace

**Files:**
- Modify: `sdks/typescript/src/client.ts` (add `OrgProviderCredentialsNamespace`, wire into `OrgsNamespace`; delete `AppProviderCredentialsNamespace` + its wiring in `AppsNamespace`)
- Modify: `sdks/typescript/src/index.ts` (regenerated OpenAPI types — `provider-credentials` now under `/v1/orgs/...`)
- Test: `sdks/typescript` build/typecheck

**Interfaces:**
- Produces: `client.orgs.providerCredentials.{ list(orgId), create(orgId, req), delete(orgId, provider) }`.
- Removes: `client.apps.providerCredentials`.

- [ ] **Step 1: Add the org namespace class** in `client.ts` (mirror the existing app class, swap `apps/{appId}` → `orgs/{orgId}`):

```ts
class OrgProviderCredentialsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(orgId: string, signal?: AbortSignal): Promise<ProviderCredentialInfo[]> {
    return this.client.request("GET", `/v1/orgs/${encodeURIComponent(orgId)}/provider-credentials`, undefined, signal);
  }
  create(orgId: string, req: CreateProviderCredentialRequest, signal?: AbortSignal): Promise<ProviderCredentialInfo> {
    return this.client.request("POST", `/v1/orgs/${encodeURIComponent(orgId)}/provider-credentials`, req, signal);
  }
  delete(orgId: string, provider: string, signal?: AbortSignal): Promise<void> {
    return this.client.request("DELETE", `/v1/orgs/${encodeURIComponent(orgId)}/provider-credentials/${encodeURIComponent(provider)}`, undefined, signal);
  }
}
```

- [ ] **Step 2: Wire into `OrgsNamespace`** (alongside `allowedModels`):

```ts
  readonly providerCredentials: OrgProviderCredentialsNamespace;
  // in constructor:
  this.providerCredentials = new OrgProviderCredentialsNamespace(client);
```

- [ ] **Step 3: Remove the app namespace** — delete `class AppProviderCredentialsNamespace`, the `readonly providerCredentials` field + constructor line in `AppsNamespace`.

- [ ] **Step 4: Regenerate OpenAPI-derived types** — the type aliases in `index.ts` come from the backend OpenAPI spec via `components["schemas"][...]`. Regenerate after Task 3's `openapi.rs` change so the new org paths/types are present and the old app-cred paths are gone:

Run: `cd sdks/typescript && npm run gen && npm run typecheck && npm run build`
Expected: `gen` (= `../scripts/regen-all.sh`) updates the generated types; `typecheck` (`tsc --noEmit`) and `build` (`tsup`) pass with no references to `apps.providerCredentials`.

- [ ] **Step 5: Commit**

```bash
git add sdks/typescript/src/client.ts sdks/typescript/src/index.ts
git commit -m "feat(sdk): org providerCredentials namespace; drop app namespace"
```

---

# Phase 2 — Frontend: provider-centric Providers page

> Depends on Phase 1 (consumes `client.orgs.providerCredentials` + the org endpoints). Verify the SDK in `dashboard/node_modules/@litegen/sdk` is the just-built one (the dashboard imports `@litegen/sdk`; rebuild/relink if needed).

### Task 6: `ModelDetail` shared drawer component

**Files:**
- Create: `dashboard/src/components/ModelDetail.tsx`
- Test: exercised via Playwright in Task 9 (component has no standalone unit harness in this repo).

**Interfaces:**
- Produces: `export default function ModelDetail({ model, onClose }: { model: ModelInfo; onClose: () => void })` — a right-side drawer that fetches `client.models.getSchema(model.id)` and renders capabilities, params, pricing, and copy-as-curl.

- [ ] **Step 1: Write the component**

```tsx
import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelInfo, ModelSchema } from '@litegen/sdk';
import { showToast } from './toast-store';

function buildCurl(model: ModelInfo): string {
  const isVideo = model.media_type === 'video';
  const endpoint = isVideo ? '/v1/videos/generations' : '/v1/images/generations';
  const body = isVideo
    ? { model: model.id, prompt: 'a photo of a cat' }
    : { model: model.id, prompt: 'a photo of a cat', n: 1 };
  return [
    `curl -X POST $LITEGEN_BASE${endpoint} \\`,
    `  -H "Authorization: Bearer $LITEGEN_KEY" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '${JSON.stringify(body)}'`,
  ].join('\n');
}

const CAP_LABELS: Record<string, string> = {
  supports_text_to_image: 'Text → image',
  supports_image_to_image: 'Image → image',
  supports_inpainting: 'Inpainting',
  supports_text_to_video: 'Text → video',
  supports_image_to_video: 'Image → video',
  supports_first_frame: 'First-frame ref',
  supports_last_frame: 'Last-frame ref',
};

export default function ModelDetail({ model, onClose }: { model: ModelInfo; onClose: () => void }) {
  const [schema, setSchema] = useState<ModelSchema | null>(null);
  const [err, setErr] = useState('');

  useEffect(() => {
    let cancelled = false;
    setSchema(null); setErr('');
    client.models.getSchema(model.id)
      .then(s => { if (!cancelled) setSchema(s); })
      .catch(e => { if (!cancelled) setErr((e as Error).message); });
    return () => { cancelled = true; };
  }, [model.id]);

  const caps = Object.entries(CAP_LABELS).filter(([k]) => (model.capabilities as Record<string, unknown>)?.[k]);
  const sizes: string[] = (model.capabilities?.supported_sizes as string[]) ?? [];
  const params = (schema?.params ?? {}) as Record<string, { kind?: string; [k: string]: unknown }>;

  const copyCurl = async () => {
    try { await navigator.clipboard.writeText(buildCurl(model)); showToast('curl command copied'); }
    catch { showToast('Failed to copy', 'error'); }
  };

  const card: React.CSSProperties = { background: '#0d1117', border: '1px solid #30363d', borderRadius: 8, padding: 14, marginBottom: 16 };
  const h4: React.CSSProperties = { margin: '0 0 10px', fontSize: 13, color: '#8b949e', textTransform: 'uppercase', letterSpacing: '0.05em' };

  return (
    <div data-testid="model-detail-panel" style={{
      position: 'fixed', top: 0, right: 0, width: 480, height: '100vh', background: '#161b22',
      borderLeft: '1px solid #30363d', zIndex: 999, overflowY: 'auto', padding: 24,
    }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
        <h3 style={{ margin: 0, color: '#e6edf3' }}>{model.name ?? model.id}</h3>
        <button className="btn btn-secondary" data-testid="model-detail-close" onClick={onClose}>Close</button>
      </div>
      <p style={{ color: '#8b949e', fontFamily: 'monospace', fontSize: 13, marginBottom: 16 }}>{model.id}</p>

      <div style={card}>
        <div style={h4}>Capabilities</div>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
          {caps.map(([, label]) => (
            <span key={label} style={{ fontSize: 12, color: '#3fb950', background: '#3fb95022', padding: '3px 8px', borderRadius: 999 }}>{label}</span>
          ))}
          {caps.length === 0 && <span style={{ color: '#6e7681', fontSize: 13 }}>—</span>}
        </div>
        {sizes.length > 0 && (
          <div style={{ marginTop: 10, color: '#8b949e', fontSize: 13 }}>
            Sizes: <span style={{ color: '#e6edf3' }}>{sizes.join(', ')}</span>
          </div>
        )}
      </div>

      <div style={card}>
        <div style={h4}>Pricing</div>
        <div style={{ color: '#e6edf3', fontSize: 14 }}>
          {model.pricing ? `$${model.pricing.base_cost_usd.toFixed(4)} base` : '—'}
          {model.pricing?.variable_pricing ? <span style={{ color: '#8b949e' }}> + variable</span> : null}
        </div>
      </div>

      <div style={card}>
        <div style={h4}>Params &amp; allowed values</div>
        {err && <div className="alert alert-error">{err}</div>}
        {!schema && !err && <div style={{ color: '#8b949e', fontSize: 13 }}>Loading…</div>}
        {schema && Object.keys(params).length === 0 && <div style={{ color: '#6e7681', fontSize: 13 }}>No tunable params.</div>}
        {schema && Object.entries(params).map(([name, spec]) => (
          <div key={name} data-testid={`param-${name}`} style={{ display: 'flex', justifyContent: 'space-between', padding: '5px 0', borderBottom: '1px solid #21262d', fontSize: 13 }}>
            <span style={{ color: '#e6edf3', fontFamily: 'monospace' }}>{name}</span>
            <span style={{ color: '#8b949e' }}>{describeSpec(spec)}</span>
          </div>
        ))}
      </div>

      <button className="btn btn-secondary" data-testid="model-detail-copy-curl" onClick={copyCurl}>Copy as curl</button>
    </div>
  );
}

/** Compact one-line summary of a param spec (kind + allowed/range + default). */
function describeSpec(spec: { kind?: string; [k: string]: unknown }): string {
  const kind = spec.kind ?? 'value';
  const allowed = (spec.allowed as string[] | undefined) ?? (spec.enum_values as string[] | undefined);
  if (Array.isArray(allowed) && allowed.length) return `${kind}: ${allowed.join(' | ')}`;
  if (spec.min != null || spec.max != null) return `${kind} ${spec.min ?? ''}–${spec.max ?? ''}`;
  if (spec.default != null) return `${kind} (default ${String(spec.default)})`;
  return kind;
}
```

- [ ] **Step 2: Typecheck**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: no errors in `ModelDetail.tsx` (if `ModelSchema.params` typing is opaque, the `Record<string, ...>` cast handles it).

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/components/ModelDetail.tsx
git commit -m "feat(dashboard): shared ModelDetail drawer (capabilities/params/pricing/curl)"
```

---

### Task 7: Rebuild `Providers.tsx` into per-provider sections

**Files:**
- Create: `dashboard/src/components/ProviderSection.tsx`
- Modify: `dashboard/src/pages/Providers.tsx` (replace Cards 1+2 with provider sections; keep the App-model-access card)
- Test: Playwright (Task 9)

**Interfaces:**
- Consumes: `client.providers.list()`, `client.models.list()`, `client.orgs.providerCredentials.{list,create,delete}`, `client.orgs.allowedModels.{get,set}`, `ModelDetail` (Task 6).
- `ProviderSection` props: `{ provider: ProviderCatalogEntry; models: ModelInfo[]; configured: boolean; displayHint?: string; enabled: Set<string>; onToggleModel: (id: string) => void; onSaveKey: (creds) => Promise<void>; onDeleteKey: () => Promise<void>; onOpenModel: (m: ModelInfo) => void }`.

- [ ] **Step 1: Write `ProviderSection.tsx`** (collapsible; header status from `configured`; key block reuses the multi-row credential form; models block has auto-save checkboxes):

```tsx
import { useState } from 'react';
import type { ProviderCatalogEntry, ModelInfo } from '@litegen/sdk';

type CredRow = { values: Record<string, string>; weight: string };
const emptyRow = (): CredRow => ({ values: {}, weight: '1' });

interface Props {
  provider: ProviderCatalogEntry;
  models: ModelInfo[];
  configured: boolean;
  displayHint?: string;
  enabled: Set<string>;
  onToggleModel: (id: string) => void;
  onOpenModel: (m: ModelInfo) => void;
  onSaveKey: (poolField: string, entries: Record<string, unknown>[]) => Promise<void>;
  onDeleteKey: () => Promise<void>;
  defaultOpen: boolean;
}

export default function ProviderSection(props: Props) {
  const { provider, models, configured, displayHint, enabled, onToggleModel, onOpenModel, onSaveKey, onDeleteKey, defaultOpen } = props;
  const [open, setOpen] = useState(defaultOpen);
  const [rows, setRows] = useState<CredRow[]>([emptyRow()]);

  const setVal = (i: number, k: string, v: string) =>
    setRows(rs => rs.map((r, idx) => idx === i ? { ...r, values: { ...r.values, [k]: v } } : r));
  const setW = (i: number, v: string) => setRows(rs => rs.map((r, idx) => idx === i ? { ...r, weight: v } : r));
  const addRow = () => setRows(rs => [...rs, emptyRow()]);
  const removeRow = (i: number) => setRows(rs => rs.length > 1 ? rs.filter((_, idx) => idx !== i) : rs);

  const save = async () => {
    const entries = rows.map(row => {
      const obj: Record<string, unknown> = {};
      for (const f of provider.fields) {
        const v = (row.values[f.key] ?? '').trim();
        if (v) obj[f.key] = v;
      }
      obj.weight = Math.max(1, parseInt(row.weight, 10) || 1);
      return obj;
    }).filter(obj => provider.fields.every(f => f.optional || typeof obj[f.key] === 'string'));
    if (entries.length === 0) return;
    await onSaveKey(provider.pool_field, entries);
    setRows([emptyRow()]);
  };

  const enabledCount = models.filter(m => enabled.has(m.id)).length;

  return (
    <div data-testid={`provider-section-${provider.name}`} style={{ border: '1px solid #30363d', borderRadius: 10, marginBottom: 12, background: '#161b22' }}>
      <button
        data-testid={`provider-toggle-${provider.name}`}
        onClick={() => setOpen(o => !o)}
        style={{ display: 'flex', width: '100%', alignItems: 'center', gap: 10, padding: '14px 16px', background: 'none', border: 'none', cursor: 'pointer', color: '#e6edf3' }}
      >
        <span style={{ color: configured ? '#3fb950' : '#6e7681' }}>{configured ? '●' : '○'}</span>
        <strong style={{ fontSize: 15 }}>{provider.name}</strong>
        <span style={{ color: '#6e7681', fontSize: 12 }}>{configured ? 'configured' : 'needs key'}</span>
        <span style={{ marginLeft: 'auto', color: '#8b949e', fontSize: 12 }}>{enabledCount}/{models.length} enabled</span>
        <span style={{ color: '#8b949e' }}>{open ? '▾' : '▸'}</span>
      </button>

      {open && (
        <div style={{ padding: '0 16px 16px' }}>
          {/* Key block (org-scoped) */}
          <div style={{ background: '#0d1117', border: '1px solid #30363d', borderRadius: 8, padding: 12, marginBottom: 14 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
              <span style={{ color: '#8b949e', fontSize: 12, textTransform: 'uppercase', letterSpacing: '0.05em' }}>API key (org-wide)</span>
              {configured && (
                <span style={{ color: '#8b949e', fontFamily: 'monospace', fontSize: 13 }}>
                  {displayHint}
                  <button className="btn btn-danger" data-testid={`provider-key-delete-${provider.name}`} onClick={onDeleteKey} style={{ marginLeft: 10, fontSize: 12, padding: '2px 8px' }}>Delete</button>
                </span>
              )}
            </div>
            {rows.map((row, i) => (
              <div key={i} style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 6 }}>
                {provider.fields.map(f => (
                  <input key={f.key} className="input"
                    data-testid={`provider-key-field-${provider.name}-${f.key}-${i}`}
                    type={f.secret ? 'password' : 'text'}
                    value={row.values[f.key] ?? ''} onChange={e => setVal(i, f.key, e.target.value)}
                    placeholder={f.optional ? `${f.label} (optional)` : f.label} style={{ maxWidth: 220 }} />
                ))}
                <input className="input" type="number" min={1} value={row.weight}
                  onChange={e => setW(i, e.target.value)} title="Weight" style={{ width: 80 }} />
                <button className="btn btn-danger" onClick={() => removeRow(i)} disabled={rows.length <= 1} style={{ fontSize: 12, padding: '4px 10px' }}>×</button>
              </div>
            ))}
            <div style={{ display: 'flex', gap: 8 }}>
              <button className="btn" onClick={addRow} style={{ fontSize: 13 }}>+ Add another key</button>
              <button className="btn btn-primary" data-testid={`provider-key-save-${provider.name}`} onClick={save}>Save key</button>
            </div>
          </div>

          {/* Models block (org-scoped enable, auto-save) */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
            {models.map(m => (
              <div key={m.id} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 8px', borderRadius: 6 }}>
                <input type="checkbox" data-testid={`provider-model-enable-${m.id}`} checked={enabled.has(m.id)} onChange={() => onToggleModel(m.id)} />
                <span style={{ color: '#e6edf3', fontSize: 14, flex: 1 }}>{m.name ?? m.id}</span>
                <span className={`badge ${m.media_type}`}>{m.media_type}</span>
                <span style={{ color: '#8b949e', fontSize: 12, width: 64, textAlign: 'right' }}>{m.pricing ? `$${m.pricing.base_cost_usd.toFixed(4)}` : '—'}</span>
                <button className="btn btn-secondary" data-testid={`provider-model-open-${m.id}`} onClick={() => onOpenModel(m)} style={{ fontSize: 12, padding: '2px 8px' }}>›</button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Rewrite `Providers.tsx`** — container that loads data, groups models by provider, renders `ProviderSection`s + the existing App-model-access card, and owns the `ModelDetail` drawer + auto-save pool logic. Keep the permission gate (owner/admin) and the App-model-access card exactly as they are today (lift them from the current file).

```tsx
import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { ProviderCatalogEntry, ModelInfo, ProviderCredentialInfo } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';
import ProviderSection from '../components/ProviderSection';
import ModelDetail from '../components/ModelDetail';
import AppModelAccessCard from '../components/AppModelAccessCard'; // extracted from today's Card 3

export default function Providers() {
  const { activeOrg, activeApp, activeOrgRole } = useTenant();
  const [catalog, setCatalog] = useState<ModelInfo[]>([]);
  const [providers, setProviders] = useState<ProviderCatalogEntry[]>([]);
  const [creds, setCreds] = useState<ProviderCredentialInfo[]>([]);
  const [enabled, setEnabled] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<ModelInfo | null>(null);

  useEffect(() => { void client.models.list().then(setCatalog).catch(() => {}); }, []);
  useEffect(() => { void client.providers.list().then(setProviders).catch(() => {}); }, []);

  const loadCreds = useCallback(async () => {
    if (!activeOrg) return;
    try { setCreds(await client.orgs.providerCredentials.list(activeOrg)); } catch { /* ignore */ }
  }, [activeOrg]);
  useEffect(() => { void loadCreds(); }, [loadCreds]);

  useEffect(() => {
    if (!activeOrg) return;
    let cancelled = false;
    void client.orgs.allowedModels.get(activeOrg).then(r => { if (!cancelled) setEnabled(new Set(r.models)); }).catch(() => {});
    return () => { cancelled = true; };
  }, [activeOrg]);

  // Auto-save a model toggle straight to the org pool.
  const toggleModel = async (id: string) => {
    if (!activeOrg) return;
    const next = new Set(enabled);
    if (next.has(id)) next.delete(id); else next.add(id);
    setEnabled(next); // optimistic
    try {
      const r = await client.orgs.allowedModels.set(activeOrg, { models: [...next] });
      setEnabled(new Set(r.models));
    } catch (err) {
      setEnabled(enabled); // revert
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const saveKey = (name: string) => async (poolField: string, entries: Record<string, unknown>[]) => {
    if (!activeOrg) return;
    try {
      await client.orgs.providerCredentials.create(activeOrg, { provider: name, credentials: { [poolField]: entries } });
      await loadCreds();
      showToast(`${name} key saved`, 'info');
    } catch (err) { if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error'); }
  };
  const deleteKey = (name: string) => async () => {
    if (!activeOrg) return;
    try { await client.orgs.providerCredentials.delete(activeOrg, name); await loadCreds(); showToast(`${name} key removed`, 'info'); }
    catch (err) { if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error'); }
  };

  if (!activeOrg) return <div style={{ padding: 24, color: '#8b949e' }}>No active organization.</div>;
  if (activeOrgRole !== 'owner' && activeOrgRole !== 'admin')
    return <div style={{ padding: '24px 32px', background: '#3d1a1a', border: '1px solid #f85149', borderRadius: 8, color: '#f85149', margin: 24 }}>You need admin or owner access to manage providers.</div>;

  const byProvider: Record<string, ModelInfo[]> = {};
  for (const m of catalog) (byProvider[m.provider] ??= []).push(m);
  const credByProvider = new Map(creds.map(c => [c.provider, c]));
  const sorted = [...providers].sort((a, b) => (credByProvider.has(b.name) ? 1 : 0) - (credByProvider.has(a.name) ? 1 : 0) || a.name.localeCompare(b.name));

  return (
    <div style={{ padding: '0 0 32px' }}>
      <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Providers</h2>
      {sorted.map(p => (
        <ProviderSection
          key={p.name}
          provider={p}
          models={byProvider[p.name] ?? []}
          configured={credByProvider.has(p.name) || p.name === 'mock'}
          displayHint={credByProvider.get(p.name)?.display_hint ?? undefined}
          enabled={enabled}
          onToggleModel={toggleModel}
          onOpenModel={setDetail}
          onSaveKey={saveKey(p.name)}
          onDeleteKey={deleteKey(p.name)}
          defaultOpen={credByProvider.has(p.name)}
        />
      ))}
      <AppModelAccessCard activeApp={activeApp} orgPool={enabled} />
      {detail && <ModelDetail model={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}
```

- [ ] **Step 3: Extract `AppModelAccessCard.tsx`** — move today's Card 3 (`dashboard/src/pages/Providers.tsx` "App model access" block) into `dashboard/src/components/AppModelAccessCard.tsx` verbatim, taking `{ activeApp, orgPool }` props. Its "select" list iterates `[...orgPool].sort()` (now always the saved pool) and it refetches `client.apps.allowedModels.get(activeApp)` on `activeApp` change — identical behavior to today, minus the coupling because `orgPool` is always persisted.

- [ ] **Step 4: Typecheck + lint**

Run: `cd dashboard && npx tsc -b --noEmit && npm run lint`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add dashboard/src/pages/Providers.tsx dashboard/src/components/ProviderSection.tsx dashboard/src/components/AppModelAccessCard.tsx
git commit -m "feat(dashboard): provider-centric Providers page with org keys + auto-save toggles"
```

---

### Task 8: Adopt `ModelDetail` on the `/models` page

**Files:**
- Modify: `dashboard/src/pages/Models.tsx` (replace the raw-JSON side panel with `<ModelDetail>`)

- [ ] **Step 1: Swap the panel** — remove the `schema`/`schemaLoading`/`schemaError`/`openDetail`/raw-`<pre>` block; on row click set a `selected: ModelInfo | null`; render `{selected && <ModelDetail model={selected} onClose={() => setSelected(null)} />}`. Keep the table + filters untouched.

- [ ] **Step 2: Typecheck**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: clean; `Models.tsx` no longer references `client.models.getSchema` directly (now inside `ModelDetail`).

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/pages/Models.tsx
git commit -m "refactor(dashboard): /models uses shared ModelDetail drawer"
```

---

### Task 9: Update multitenant e2e for org keys + new UI

**Files:**
- Modify: `dashboard/e2e-mt/multitenant.spec.ts`

**Interfaces:**
- Consumes: the new testids — `provider-section-{name}`, `provider-toggle-{name}`, `provider-key-field-mock-key-0`, `provider-key-save-mock`, `provider-model-enable-{id}`, `provider-model-open-{id}`, `model-detail-panel`, `model-detail-copy-curl`.

- [ ] **Step 1: Replace the stale credential steps** (current lines ~69-79 drive the removed `/organization` app-cred UI). New flow on `/providers`:

```ts
// Configure an ORG-level mock key.
await page.goto('/providers');
await page.locator('[data-testid="provider-toggle-mock"]').click();
await page.locator('[data-testid="provider-key-field-mock-key-0"]').fill('sk-mock-abcd1234');
await page.locator('[data-testid="provider-key-save-mock"]').click();
await expect(page.locator('[data-testid="provider-section-mock"]')).toContainText('configured', { timeout: 15_000 });
await expect(page.locator('body')).not.toContainText('sk-mock-abcd1234');

// Enabling a model auto-saves to the org pool (no save button).
await page.locator('[data-testid="provider-model-enable-mock/image-gen"]').check();
await expect(page.locator('[data-testid="provider-model-enable-mock/image-gen"]')).toBeChecked();

// Model detail drawer shows the feature list.
await page.locator('[data-testid="provider-model-open-mock/image-gen"]').click();
await expect(page.locator('[data-testid="model-detail-panel"]')).toBeVisible();
await expect(page.locator('[data-testid="model-detail-panel"]')).toContainText('Pricing');
```

- [ ] **Step 2: Add a second-app key-reuse assertion** — after creating a 2nd app, mint a key under it and POST a real generation; it must succeed using the org key (proves org-scoped resolution). Reuse the existing `request.post('/v1/images/generations', { model: 'mock/image-gen' })` pattern already in the spec.

- [ ] **Step 3: Run the multitenant e2e**

Run: `cd litegen-core && cargo build --release --bin litegen && cd ../dashboard && npm run test:e2e:mt`
Expected: PASS (the spec boots the release binary + vite per `playwright.multitenant.config.ts`).

- [ ] **Step 4: Commit**

```bash
git add dashboard/e2e-mt/multitenant.spec.ts
git commit -m "test(e2e): org provider keys + provider-section UI + model detail"
```

---

## Self-Review notes (for the implementer)

- **Spec coverage:** Task 1=DB table+migration; Task 2=trait/impls; Task 3=routes; Task 4=resolution; Task 5=SDK; Task 6=ModelDetail; Task 7=per-provider page + auto-save (kills coupling bug); Task 8=/models adoption; Task 9=e2e. All spec sections map to a task.
- **Verification:** after the whole plan, run the `verify` skill against `/providers` (the multitenant harness) — configure a key, toggle a model (confirm it persists with no Save button), open the detail drawer, and confirm a second app generates using the org key.
- **Watch-outs:** (a) the dashboard imports the built `@litegen/sdk` — rebuild/relink after Task 5 before Phase 2. (b) Mirror the exact `created_at` decode types from the existing app DB impls. (c) If `openapi.rs` or generated SDK types pin the old app-cred paths, regenerate both.
