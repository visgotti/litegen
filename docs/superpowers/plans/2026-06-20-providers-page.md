# Providers Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a dedicated `/providers` dashboard page with org-level model pool configuration, app credential management (lifted from Organization.tsx), and per-app model access scoping.

**Architecture:** Three new DB tables store org/app model allowlists; four new REST endpoints expose them; the SDK gains two new namespaces; a new Providers.tsx page renders three cards consuming those endpoints. Provider credential management is lifted verbatim from Organization.tsx into the new page, and Organization.tsx's credential card is removed.

**Tech Stack:** Rust/axum/sqlx (backend), TypeScript SDK (client.ts), React/TSX (dashboard)

## Global Constraints

- SQLite migrations: `litegen-core/migrations/sqlite/` — `?` placeholders, `datetime('now')`
- Postgres migrations: `litegen-core/migrations/postgres/` — `$N` placeholders, `NOW()`
- Migration filenames: next sequential number after `20240101000011` → use `20240101000012`
- DB trait default impls (no-op): `litegen-core/src/db/trait_def.rs`
- DB impls: `litegen-core/src/db/sqlite.rs` and `litegen-core/src/db/postgres.rs`
- Permission gate: use existing `require_member_perm` + existing `OrgRead`/`OrgWrite`/`AppRead`/`AppWrite` variants from `crate::auth::permissions::Permission`
- Route registration: `create_router` in `litegen-core/src/api/handlers/mod.rs` (session-authenticated block, alongside existing `/v1/orgs` and `/v1/apps` routes, lines ~2059–2083)
- All new handlers go in `litegen-core/src/api/handlers/orgs.rs` (existing file for org/app handlers)
- SDK: `sdks/typescript/src/client.ts` — follow existing namespace class pattern
- Dashboard: `dashboard/src/pages/` — follow existing page pattern (inline styles, `useTenant` hook, `showToast` for errors)
- No new npm packages; no new Rust crates
- Do NOT add Co-Authored-By trailers to commits

---

## File Map

| File | Action | Purpose |
|------|--------|---------|
| `litegen-core/migrations/sqlite/20240101000012_model_allowlists.sql` | Create | SQLite tables for org/app model allowlists |
| `litegen-core/migrations/postgres/20240101000012_model_allowlists.sql` | Create | Postgres tables for org/app model allowlists |
| `litegen-core/src/types/mod.rs` | Modify | Add `OrgAllowedModels`, `AppModelAccess`, `SetOrgAllowedModelsRequest`, `SetAppModelAccessRequest` types |
| `litegen-core/src/db/trait_def.rs` | Modify | Add 4 new trait methods with no-op defaults |
| `litegen-core/src/db/sqlite.rs` | Modify | Implement the 4 new trait methods for SQLite |
| `litegen-core/src/db/postgres.rs` | Modify | Implement the 4 new trait methods for Postgres |
| `litegen-core/src/api/handlers/orgs.rs` | Modify | Add 4 new handler functions |
| `litegen-core/src/api/handlers/mod.rs` | Modify | Register 4 new routes |
| `sdks/typescript/src/client.ts` | Modify | Add `OrgAllowedModelsNamespace`, `AppModelAccessNamespace`, wire into `OrgsNamespace`/`AppsNamespace` |
| `dashboard/src/pages/Providers.tsx` | Create | New page: 3 cards (org pool, app creds, app model access) |
| `dashboard/src/pages/Organization.tsx` | Modify | Remove provider credentials card (lines 312–443) |
| `dashboard/src/App.tsx` | Modify | Add Providers nav entry + route |

---

## Task 1: DB Migrations

**Files:**
- Create: `litegen-core/migrations/sqlite/20240101000012_model_allowlists.sql`
- Create: `litegen-core/migrations/postgres/20240101000012_model_allowlists.sql`

**Interfaces:**
- Produces: `org_allowed_models`, `app_model_access`, `app_model_access_ids` tables in both DBs

- [ ] **Step 1: Write the SQLite migration**

`litegen-core/migrations/sqlite/20240101000012_model_allowlists.sql`:
```sql
-- Org-level model pool: which catalog models this org has enabled.
CREATE TABLE org_allowed_models (
    org_id   TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (org_id, model_id)
);

-- Per-app model access mode (one row per app, defaults to 'all').
CREATE TABLE app_model_access (
    app_id TEXT NOT NULL PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    mode   TEXT NOT NULL DEFAULT 'all'
);

-- Model IDs selected when an app's mode = 'select'.
CREATE TABLE app_model_access_ids (
    app_id   TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (app_id, model_id)
);
```

- [ ] **Step 2: Write the Postgres migration**

`litegen-core/migrations/postgres/20240101000012_model_allowlists.sql`:
```sql
CREATE TABLE org_allowed_models (
    org_id   TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (org_id, model_id)
);

CREATE TABLE app_model_access (
    app_id TEXT NOT NULL PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    mode   TEXT NOT NULL DEFAULT 'all'
);

CREATE TABLE app_model_access_ids (
    app_id   TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (app_id, model_id)
);
```

- [ ] **Step 3: Verify migrations compile (SQLite path)**

```bash
cd litegen-core
cargo test --test multitenant_api -- --nocapture 2>&1 | head -20
```
Expected: tests run (some may fail on unimplemented methods — that's fine; what matters is no migration error).

- [ ] **Step 4: Commit**

```bash
git add litegen-core/migrations/
git commit -m "feat(db): add org/app model allowlist migrations"
```

---

## Task 2: Rust Types + DB Trait + SQLite/Postgres Implementations

**Files:**
- Modify: `litegen-core/src/types/mod.rs`
- Modify: `litegen-core/src/db/trait_def.rs`
- Modify: `litegen-core/src/db/sqlite.rs`
- Modify: `litegen-core/src/db/postgres.rs`

**Interfaces:**
- Produces:
  - `OrgAllowedModels { models: Vec<String> }` — GET response
  - `AppModelAccess { mode: String, models: Vec<String> }` — GET response
  - `SetOrgAllowedModelsRequest { models: Vec<String> }` — PUT body
  - `SetAppModelAccessRequest { mode: String, models: Vec<String> }` — PUT body
  - `db.get_org_allowed_models(org_id) -> Result<Vec<String>, sqlx::Error>`
  - `db.set_org_allowed_models(org_id, models) -> Result<(), sqlx::Error>`
  - `db.get_app_model_access(app_id) -> Result<(String, Vec<String>), sqlx::Error>` — returns `(mode, model_ids)`
  - `db.set_app_model_access(app_id, mode, models) -> Result<(), sqlx::Error>`

- [ ] **Step 1: Add types to `litegen-core/src/types/mod.rs`**

Find the `// ─── Tenancy` section (around line 596) and add after the `AppStorageInfo` struct:

```rust
// ─── Model Allowlists ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrgAllowedModels {
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AppModelAccess {
    /// "all" = inherit everything in the org pool. "select" = only `models`.
    pub mode: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetOrgAllowedModelsRequest {
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetAppModelAccessRequest {
    /// Must be "all" or "select".
    pub mode: String,
    #[serde(default)]
    pub models: Vec<String>,
}
```

- [ ] **Step 2: Add trait methods to `litegen-core/src/db/trait_def.rs`**

Find the `// ─── Per-app BYO storage config` section and add a new section after `delete_app_storage`:

```rust
// ─── Model Allowlists ───────────────────────────────────────────────────────

async fn get_org_allowed_models(&self, _org_id: &str) -> Result<Vec<String>, sqlx::Error> {
    Ok(vec![])
}

async fn set_org_allowed_models(
    &self,
    _org_id: &str,
    _models: &[String],
) -> Result<(), sqlx::Error> {
    Ok(())
}

/// Returns `(mode, model_ids)`. `mode` defaults to `"all"` if no row exists.
async fn get_app_model_access(
    &self,
    _app_id: &str,
) -> Result<(String, Vec<String>), sqlx::Error> {
    Ok(("all".to_string(), vec![]))
}

async fn set_app_model_access(
    &self,
    _app_id: &str,
    _mode: &str,
    _models: &[String],
) -> Result<(), sqlx::Error> {
    Ok(())
}
```

- [ ] **Step 3: Implement in `litegen-core/src/db/sqlite.rs`**

Add after the `delete_app_storage` implementation (around line 1601):

```rust
// ─── Model Allowlists ───────────────────────────────────────────────────────

async fn get_org_allowed_models(&self, org_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT model_id FROM org_allowed_models WHERE org_id = ? ORDER BY model_id")
            .bind(org_id)
            .fetch_all(&self.pool)
            .await?;
    Ok(rows.into_iter().map(|(m,)| m).collect())
}

async fn set_org_allowed_models(
    &self,
    org_id: &str,
    models: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = self.pool.begin().await?;
    sqlx::query("DELETE FROM org_allowed_models WHERE org_id = ?")
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
    for model_id in models {
        sqlx::query("INSERT INTO org_allowed_models (org_id, model_id) VALUES (?, ?)")
            .bind(org_id)
            .bind(model_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

async fn get_app_model_access(
    &self,
    app_id: &str,
) -> Result<(String, Vec<String>), sqlx::Error> {
    let mode_row: Option<(String,)> =
        sqlx::query_as("SELECT mode FROM app_model_access WHERE app_id = ?")
            .bind(app_id)
            .fetch_optional(&self.pool)
            .await?;
    let mode = mode_row.map(|(m,)| m).unwrap_or_else(|| "all".to_string());
    let ids: Vec<(String,)> =
        sqlx::query_as("SELECT model_id FROM app_model_access_ids WHERE app_id = ? ORDER BY model_id")
            .bind(app_id)
            .fetch_all(&self.pool)
            .await?;
    Ok((mode, ids.into_iter().map(|(m,)| m).collect()))
}

async fn set_app_model_access(
    &self,
    app_id: &str,
    mode: &str,
    models: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = self.pool.begin().await?;
    sqlx::query(
        "INSERT INTO app_model_access (app_id, mode) VALUES (?, ?) \
         ON CONFLICT (app_id) DO UPDATE SET mode = excluded.mode",
    )
    .bind(app_id)
    .bind(mode)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM app_model_access_ids WHERE app_id = ?")
        .bind(app_id)
        .execute(&mut *tx)
        .await?;
    for model_id in models {
        sqlx::query("INSERT INTO app_model_access_ids (app_id, model_id) VALUES (?, ?)")
            .bind(app_id)
            .bind(model_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}
```

- [ ] **Step 4: Implement in `litegen-core/src/db/postgres.rs`**

Add after the `delete_app_storage` implementation (around line 1619) — same logic but `$1`/`$2` placeholders and `NOW()`:

```rust
// ─── Model Allowlists ───────────────────────────────────────────────────────

async fn get_org_allowed_models(&self, org_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT model_id FROM org_allowed_models WHERE org_id = $1 ORDER BY model_id")
            .bind(org_id)
            .fetch_all(&self.pool)
            .await?;
    Ok(rows.into_iter().map(|(m,)| m).collect())
}

async fn set_org_allowed_models(
    &self,
    org_id: &str,
    models: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = self.pool.begin().await?;
    sqlx::query("DELETE FROM org_allowed_models WHERE org_id = $1")
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
    for model_id in models {
        sqlx::query("INSERT INTO org_allowed_models (org_id, model_id) VALUES ($1, $2)")
            .bind(org_id)
            .bind(model_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

async fn get_app_model_access(
    &self,
    app_id: &str,
) -> Result<(String, Vec<String>), sqlx::Error> {
    let mode_row: Option<(String,)> =
        sqlx::query_as("SELECT mode FROM app_model_access WHERE app_id = $1")
            .bind(app_id)
            .fetch_optional(&self.pool)
            .await?;
    let mode = mode_row.map(|(m,)| m).unwrap_or_else(|| "all".to_string());
    let ids: Vec<(String,)> =
        sqlx::query_as("SELECT model_id FROM app_model_access_ids WHERE app_id = $1 ORDER BY model_id")
            .bind(app_id)
            .fetch_all(&self.pool)
            .await?;
    Ok((mode, ids.into_iter().map(|(m,)| m).collect()))
}

async fn set_app_model_access(
    &self,
    app_id: &str,
    mode: &str,
    models: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = self.pool.begin().await?;
    sqlx::query(
        "INSERT INTO app_model_access (app_id, mode) VALUES ($1, $2) \
         ON CONFLICT (app_id) DO UPDATE SET mode = EXCLUDED.mode",
    )
    .bind(app_id)
    .bind(mode)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM app_model_access_ids WHERE app_id = $1")
        .bind(app_id)
        .execute(&mut *tx)
        .await?;
    for model_id in models {
        sqlx::query("INSERT INTO app_model_access_ids (app_id, model_id) VALUES ($1, $2)")
            .bind(app_id)
            .bind(model_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}
```

- [ ] **Step 5: Verify it compiles**

```bash
cd litegen-core && cargo build 2>&1 | grep -E "^error"
```
Expected: no output (zero errors).

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/types/mod.rs litegen-core/src/db/
git commit -m "feat(db): model allowlist types and DB trait/impls"
```

---

## Task 3: Backend Handlers + Route Registration

**Files:**
- Modify: `litegen-core/src/api/handlers/orgs.rs`
- Modify: `litegen-core/src/api/handlers/mod.rs`

**Interfaces:**
- Consumes: `db.get_org_allowed_models`, `db.set_org_allowed_models`, `db.get_app_model_access`, `db.set_app_model_access`, `OrgAllowedModels`, `AppModelAccess`, `SetOrgAllowedModelsRequest`, `SetAppModelAccessRequest`
- Produces: 
  - `GET /v1/orgs/{id}/allowed-models` → `OrgAllowedModels`
  - `PUT /v1/orgs/{id}/allowed-models` → `OrgAllowedModels`
  - `GET /v1/apps/{app_id}/allowed-models` → `AppModelAccess`
  - `PUT /v1/apps/{app_id}/allowed-models` → `AppModelAccess`

- [ ] **Step 1: Add imports to `orgs.rs`**

Find the existing `use crate::types::{...}` import block at the top of `orgs.rs` and add the four new types:

```rust
use crate::types::{
    Application, Invitation, Organization, OrganizationMember, ProviderCredentialInfo, Role,
    OrgAllowedModels, AppModelAccess, SetOrgAllowedModelsRequest, SetAppModelAccessRequest,
};
```

- [ ] **Step 2: Add the four handler functions to `orgs.rs`**

Add at the end of the file (after `delete_app_storage`):

```rust
// ─── GET /v1/orgs/{id}/allowed-models ──────────────────────────────────────

pub async fn get_org_allowed_models(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(org_id): Path<String>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::OrgRead).await {
        return resp;
    }
    match state.db.get_org_allowed_models(&org_id).await {
        Ok(models) => (StatusCode::OK, Json(OrgAllowedModels { models })).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── PUT /v1/orgs/{id}/allowed-models ──────────────────────────────────────

pub async fn put_org_allowed_models(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(org_id): Path<String>,
    Json(body): Json<SetOrgAllowedModelsRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::OrgWrite).await {
        return resp;
    }
    match state.db.set_org_allowed_models(&org_id, &body.models).await {
        Ok(()) => (StatusCode::OK, Json(OrgAllowedModels { models: body.models })).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── GET /v1/apps/{app_id}/allowed-models ──────────────────────────────────

pub async fn get_app_allowed_models(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::AppRead).await {
        return resp;
    }
    match state.db.get_app_model_access(&app_id).await {
        Ok((mode, models)) => (StatusCode::OK, Json(AppModelAccess { mode, models })).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── PUT /v1/apps/{app_id}/allowed-models ──────────────────────────────────

pub async fn put_app_allowed_models(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
    Json(body): Json<SetAppModelAccessRequest>,
) -> Response {
    let (app, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::AppWrite).await {
        return resp;
    }
    if body.mode != "all" && body.mode != "select" {
        return err(StatusCode::BAD_REQUEST, "invalid_mode", "mode must be 'all' or 'select'");
    }
    // When mode = "select", validate every requested model is in the org pool.
    if body.mode == "select" && !body.models.is_empty() {
        let org_pool = match state.db.get_org_allowed_models(&app.org_id).await {
            Ok(m) => m,
            Err(e) => return internal_error(&e.to_string()),
        };
        let org_set: std::collections::HashSet<&str> = org_pool.iter().map(|s| s.as_str()).collect();
        for model_id in &body.models {
            if !org_set.contains(model_id.as_str()) {
                return err(
                    StatusCode::BAD_REQUEST,
                    "model_not_in_org_pool",
                    &format!("model '{model_id}' is not in the org's allowed pool"),
                );
            }
        }
    }
    let models_to_store = if body.mode == "all" { vec![] } else { body.models.clone() };
    match state.db.set_app_model_access(&app_id, &body.mode, &models_to_store).await {
        Ok(()) => (StatusCode::OK, Json(AppModelAccess { mode: body.mode, models: body.models })).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}
```

- [ ] **Step 3: Register the four routes in `handlers/mod.rs`**

Find the block ending with the `/v1/apps/{app_id}/storage` route (around line 2079) and add after it, before the `.layer(middleware...)` call:

```rust
.route(
    "/v1/orgs/{id}/allowed-models",
    get(orgs::get_org_allowed_models).put(orgs::put_org_allowed_models),
)
.route(
    "/v1/apps/{app_id}/allowed-models",
    get(orgs::get_app_allowed_models).put(orgs::put_app_allowed_models),
)
```

- [ ] **Step 4: Verify it compiles**

```bash
cd litegen-core && cargo build 2>&1 | grep -E "^error"
```
Expected: no output.

- [ ] **Step 5: Smoke-test with curl (start the server first)**

```bash
# In one terminal: cargo run
# In another:
curl -s -X GET http://localhost:3000/v1/orgs/test-org/allowed-models \
  -H "Authorization: Bearer $MASTER_KEY" | jq .
# Expected: {"models":[]}

curl -s -X PUT http://localhost:3000/v1/orgs/test-org/allowed-models \
  -H "Authorization: Bearer $MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{"models":["openai/dall-e-3"]}' | jq .
# Expected: {"models":["openai/dall-e-3"]}
```

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/api/handlers/
git commit -m "feat(api): GET/PUT org and app allowed-models endpoints"
```

---

## Task 4: SDK Client Methods

**Files:**
- Modify: `sdks/typescript/src/client.ts`

**Interfaces:**
- Consumes: `GET/PUT /v1/orgs/{id}/allowed-models`, `GET/PUT /v1/apps/{app_id}/allowed-models`
- Produces:
  - `client.orgs.allowedModels.get(orgId)` → `Promise<{ models: string[] }>`
  - `client.orgs.allowedModels.set(orgId, req)` → `Promise<{ models: string[] }>`
  - `client.apps.allowedModels.get(appId)` → `Promise<{ mode: string; models: string[] }>`
  - `client.apps.allowedModels.set(appId, req)` → `Promise<{ mode: string; models: string[] }>`

- [ ] **Step 1: Add TypeScript types**

Find the existing type/interface declarations near the top of `client.ts` (look for `ProviderCredentialInfo`, `AppStorageInfo`, etc.) and add:

```typescript
export interface OrgAllowedModels {
  models: string[];
}

export interface AppModelAccess {
  mode: 'all' | 'select';
  models: string[];
}

export interface SetOrgAllowedModelsRequest {
  models: string[];
}

export interface SetAppModelAccessRequest {
  mode: 'all' | 'select';
  models?: string[];
}
```

- [ ] **Step 2: Add `OrgAllowedModelsNamespace` class**

Add after the `OrgMembersNamespace` class (before `OrgsNamespace`):

```typescript
class OrgAllowedModelsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(orgId: string, signal?: AbortSignal): Promise<OrgAllowedModels> {
    return this.client.request(
      'GET',
      `/v1/orgs/${encodeURIComponent(orgId)}/allowed-models`,
      undefined,
      signal,
    );
  }

  set(orgId: string, req: SetOrgAllowedModelsRequest, signal?: AbortSignal): Promise<OrgAllowedModels> {
    return this.client.request(
      'PUT',
      `/v1/orgs/${encodeURIComponent(orgId)}/allowed-models`,
      req,
      signal,
    );
  }
}
```

- [ ] **Step 3: Wire `allowedModels` into `OrgsNamespace`**

In `OrgsNamespace` (around line 971), add:

```typescript
class OrgsNamespace {
  readonly members: OrgMembersNamespace;
  readonly apps: OrgAppsNamespace;
  readonly allowedModels: OrgAllowedModelsNamespace;  // add this line

  constructor(private readonly client: LiteGenClient) {
    this.members = new OrgMembersNamespace(client);
    this.apps = new OrgAppsNamespace(client);
    this.allowedModels = new OrgAllowedModelsNamespace(client);  // add this line
  }
  // ... rest unchanged
```

- [ ] **Step 4: Add `AppModelAccessNamespace` class**

Add after `AppStorageNamespace` (before `AppsNamespace`):

```typescript
class AppModelAccessNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(appId: string, signal?: AbortSignal): Promise<AppModelAccess> {
    return this.client.request(
      'GET',
      `/v1/apps/${encodeURIComponent(appId)}/allowed-models`,
      undefined,
      signal,
    );
  }

  set(appId: string, req: SetAppModelAccessRequest, signal?: AbortSignal): Promise<AppModelAccess> {
    return this.client.request(
      'PUT',
      `/v1/apps/${encodeURIComponent(appId)}/allowed-models`,
      req,
      signal,
    );
  }
}
```

- [ ] **Step 5: Wire `allowedModels` into `AppsNamespace`**

In `AppsNamespace` (around line 1075):

```typescript
class AppsNamespace {
  readonly providerCredentials: AppProviderCredentialsNamespace;
  readonly storage: AppStorageNamespace;
  readonly allowedModels: AppModelAccessNamespace;  // add this line

  constructor(private readonly client: LiteGenClient) {
    this.providerCredentials = new AppProviderCredentialsNamespace(client);
    this.storage = new AppStorageNamespace(client);
    this.allowedModels = new AppModelAccessNamespace(client);  // add this line
  }
  // ... rest unchanged
```

- [ ] **Step 6: Verify TypeScript compiles**

```bash
cd sdks/typescript && npm run build 2>&1 | grep -E "error TS"
```
Expected: no output.

- [ ] **Step 7: Commit**

```bash
git add sdks/typescript/src/client.ts
git commit -m "feat(sdk): org/app allowed-models client methods"
```

---

## Task 5: Providers.tsx + Organization.tsx cleanup + Nav

**Files:**
- Create: `dashboard/src/pages/Providers.tsx`
- Modify: `dashboard/src/pages/Organization.tsx`
- Modify: `dashboard/src/App.tsx`

**Interfaces:**
- Consumes: `client.orgs.allowedModels.get/set`, `client.apps.allowedModels.get/set`, `client.apps.providerCredentials.*`, `client.providers.list()`, `client.models.list()`, `useTenant()`, `showToast`
- Produces: `/providers` page; Organization.tsx without provider credentials card; Providers nav entry

- [ ] **Step 1: Create `dashboard/src/pages/Providers.tsx`**

```tsx
import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type {
  ProviderCredentialInfo,
  ProviderCatalogEntry,
  ModelInfo,
} from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';

export default function Providers() {
  const { orgs, activeOrg, activeApp, activeOrgRole } = useTenant();

  // ── Org model pool ────────────────────────────────────────────────────────
  const [catalog, setCatalog] = useState<ModelInfo[]>([]);
  const [orgModels, setOrgModels] = useState<Set<string>>(new Set());
  const [orgModelsDirty, setOrgModelsDirty] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void client.models.list().then(models => {
      if (cancelled) return;
      setCatalog(models);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (!activeOrg) return;
    let cancelled = false;
    void client.orgs.allowedModels.get(activeOrg).then(r => {
      if (cancelled) return;
      setOrgModels(new Set(r.models));
      setOrgModelsDirty(false);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [activeOrg]);

  const toggleOrgModel = (modelId: string) => {
    setOrgModels(prev => {
      const next = new Set(prev);
      if (next.has(modelId)) next.delete(modelId); else next.add(modelId);
      return next;
    });
    setOrgModelsDirty(true);
  };

  const saveOrgModels = async () => {
    if (!activeOrg) return;
    try {
      const r = await client.orgs.allowedModels.set(activeOrg, { models: [...orgModels] });
      setOrgModels(new Set(r.models));
      setOrgModelsDirty(false);
      showToast('Org model pool saved', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  // Group catalog by provider prefix (e.g. "openai/dall-e-3" → "openai")
  const byProvider: Record<string, ModelInfo[]> = {};
  for (const m of catalog) {
    const provider = m.id.split('/')[0] ?? m.id;
    (byProvider[provider] ??= []).push(m);
  }

  // ── App credentials ───────────────────────────────────────────────────────
  const [creds, setCreds] = useState<ProviderCredentialInfo[]>([]);
  const [providerCatalog, setProviderCatalog] = useState<ProviderCatalogEntry[]>([]);
  const [credProvider, setCredProvider] = useState('');
  type CredRow = { values: Record<string, string>; weight: string };
  const emptyRow = (): CredRow => ({ values: {}, weight: '1' });
  const [credRows, setCredRows] = useState<CredRow[]>([emptyRow()]);

  const loadCreds = useCallback(async () => {
    if (!activeApp) return [] as ProviderCredentialInfo[];
    try { return await client.apps.providerCredentials.list(activeApp); }
    catch { return [] as ProviderCredentialInfo[]; }
  }, [activeApp]);

  useEffect(() => {
    let cancelled = false;
    void loadCreds().then(list => { if (!cancelled) setCreds(list); });
    return () => { cancelled = true; };
  }, [loadCreds]);

  useEffect(() => {
    let cancelled = false;
    void client.providers.list()
      .then(list => {
        if (cancelled) return;
        setProviderCatalog(list);
        setCredProvider(prev => prev || list[0]?.name || '');
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);

  const selectedProvider = providerCatalog.find(p => p.name === credProvider) ?? null;
  const changeProvider = (name: string) => { setCredProvider(name); setCredRows([emptyRow()]); };
  const setRowValue = (i: number, key: string, value: string) =>
    setCredRows(rows => rows.map((r, idx) => idx === i ? { ...r, values: { ...r.values, [key]: value } } : r));
  const setRowWeight = (i: number, value: string) =>
    setCredRows(rows => rows.map((r, idx) => idx === i ? { ...r, weight: value } : r));
  const addRow = () => setCredRows(rows => [...rows, emptyRow()]);
  const removeRow = (i: number) =>
    setCredRows(rows => rows.length > 1 ? rows.filter((_, idx) => idx !== i) : rows);

  const addCred = async () => {
    if (!activeApp || !selectedProvider) return;
    const provider = selectedProvider;
    const entries = credRows
      .map(row => {
        const obj: Record<string, unknown> = {};
        for (const f of provider.fields) {
          const v = (row.values[f.key] ?? '').trim();
          if (v) obj[f.key] = v;
        }
        obj.weight = Math.max(1, parseInt(row.weight, 10) || 1);
        return obj;
      })
      .filter(obj => provider.fields.every(f => f.optional || typeof obj[f.key] === 'string'));
    if (entries.length === 0) { showToast('Fill in at least one credential', 'error'); return; }
    try {
      await client.apps.providerCredentials.create(activeApp, {
        provider: provider.name,
        credentials: { [provider.pool_field]: entries },
      });
      setCredRows([emptyRow()]);
      setCreds(await loadCreds());
      showToast(`Provider ${entries.length === 1 ? 'credential' : `credentials (${entries.length})`} saved`, 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const deleteCred = async (provider: string) => {
    if (!activeApp) return;
    try {
      await client.apps.providerCredentials.delete(activeApp, provider);
      setCreds(await loadCreds());
      showToast('Provider credential removed', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error');
    }
  };

  // ── App model access ──────────────────────────────────────────────────────
  const [appModelMode, setAppModelMode] = useState<'all' | 'select'>('all');
  const [appModelIds, setAppModelIds] = useState<Set<string>>(new Set());
  const [appModelDirty, setAppModelDirty] = useState(false);

  useEffect(() => {
    if (!activeApp) return;
    let cancelled = false;
    void client.apps.allowedModels.get(activeApp).then(r => {
      if (cancelled) return;
      setAppModelMode(r.mode as 'all' | 'select');
      setAppModelIds(new Set(r.models));
      setAppModelDirty(false);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [activeApp]);

  const saveAppModels = async () => {
    if (!activeApp) return;
    try {
      const r = await client.apps.allowedModels.set(activeApp, {
        mode: appModelMode,
        models: appModelMode === 'select' ? [...appModelIds] : [],
      });
      setAppModelMode(r.mode as 'all' | 'select');
      setAppModelIds(new Set(r.models));
      setAppModelDirty(false);
      showToast('App model access saved', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const toggleAppModel = (modelId: string) => {
    setAppModelIds(prev => {
      const next = new Set(prev);
      if (next.has(modelId)) next.delete(modelId); else next.add(modelId);
      return next;
    });
    setAppModelDirty(true);
  };

  // ── Permission gate ───────────────────────────────────────────────────────
  if (!activeOrg) {
    return (
      <div style={{ padding: 24, color: '#8b949e' }}>
        No active organization.
      </div>
    );
  }

  if (activeOrgRole !== 'owner' && activeOrgRole !== 'admin') {
    return (
      <div style={{
        padding: '24px 32px', background: '#3d1a1a', border: '1px solid #f85149',
        borderRadius: 8, color: '#f85149', margin: 24,
      }}>
        You need admin or owner access to manage providers.
      </div>
    );
  }

  const cardStyle: React.CSSProperties = {
    background: '#161b22', border: '1px solid #30363d', borderRadius: 10, padding: 20, marginBottom: 24,
  };
  const sectionTitle: React.CSSProperties = { margin: '0 0 16px', color: '#e6edf3', fontSize: 18, fontWeight: 600 };
  const badge = (text: string, color: string) => (
    <span style={{ fontSize: 11, fontWeight: 600, padding: '2px 7px', borderRadius: 999, background: color + '22', color, marginLeft: 6 }}>
      {text}
    </span>
  );

  // ── Render ────────────────────────────────────────────────────────────────
  return (
    <div style={{ padding: '0 0 32px' }}>
      <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Providers</h2>

      {/* Card 1 — Org model pool */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Org model pool</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Models enabled for this org. Apps may only use models from this pool.
          {orgModels.size === 0 && (
            <span style={{ marginLeft: 8, color: '#e3b341', fontWeight: 600 }}>
              ⚠ No models enabled — apps have no access.
            </span>
          )}
        </p>
        {catalog.length === 0 ? (
          <div style={{ color: '#8b949e', fontSize: 14 }}>Loading model catalog…</div>
        ) : (
          Object.entries(byProvider).map(([provider, models]) => (
            <div key={provider} style={{ marginBottom: 16 }}>
              <div style={{ color: '#8b949e', fontSize: 12, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>
                {provider}
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                {models.map(m => (
                  <label key={m.id} style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', padding: '6px 10px', borderRadius: 6, background: orgModels.has(m.id) ? '#1a3f5c22' : 'transparent' }}>
                    <input
                      type="checkbox"
                      checked={orgModels.has(m.id)}
                      onChange={() => toggleOrgModel(m.id)}
                    />
                    <span style={{ color: '#e6edf3', fontSize: 14 }}>{m.name ?? m.id}</span>
                    {badge(m.media_type, m.media_type === 'video' ? '#d2a8ff' : '#3fb950')}
                  </label>
                ))}
              </div>
            </div>
          ))
        )}
        <button
          className="btn btn-primary"
          onClick={saveOrgModels}
          disabled={!orgModelsDirty}
          style={{ marginTop: 8 }}
        >
          Save org model pool
        </button>
      </div>

      {/* Card 2 — App credentials */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>App credentials</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Provider API keys scoped to the active app{activeApp ? '' : ' — select an app to manage credentials'}.
        </p>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 16 }}>
          {creds.length === 0 && (
            <div style={{ color: '#8b949e', fontSize: 14 }}>No provider credentials configured.</div>
          )}
          {creds.map(c => (
            <div
              key={c.provider}
              data-testid={`provider-cred-row-${c.provider}`}
              style={{
                display: 'flex', alignItems: 'center', justifyContent: 'space-between',
                padding: '10px 14px', background: '#0d1117', border: '1px solid #30363d', borderRadius: 8,
              }}
            >
              <span style={{ color: '#e6edf3', fontSize: 14 }}>
                <strong>{c.provider}</strong>
                {c.display_hint && (
                  <span style={{ color: '#8b949e', marginLeft: 8, fontFamily: 'monospace', fontSize: 13 }}>
                    {c.display_hint}
                  </span>
                )}
              </span>
              <button
                className="btn btn-danger"
                data-testid={`provider-cred-delete-${c.provider}`}
                onClick={() => deleteCred(c.provider)}
                style={{ fontSize: 12, padding: '4px 10px' }}
              >
                Delete
              </button>
            </div>
          ))}
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
            <label style={{ color: '#8b949e', fontSize: 13 }}>Provider</label>
            <select
              className="input"
              data-testid="provider-cred-provider"
              value={credProvider}
              onChange={e => changeProvider(e.target.value)}
              disabled={!activeApp || providerCatalog.length === 0}
              style={{ maxWidth: 220 }}
            >
              {providerCatalog.map(p => (
                <option key={p.name} value={p.name}>{p.name}</option>
              ))}
            </select>
            {selectedProvider && selectedProvider.modalities.length > 0 && (
              <span style={{ color: '#6e7681', fontSize: 12 }}>
                {selectedProvider.modalities.join(' + ')}
              </span>
            )}
          </div>
          {selectedProvider && (
            <>
              {credRows.map((row, i) => (
                <div
                  key={i}
                  data-testid={`provider-cred-input-row-${i}`}
                  style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}
                >
                  {selectedProvider.fields.map(f => (
                    <input
                      key={f.key}
                      className="input"
                      data-testid={`provider-cred-field-${f.key}-${i}`}
                      type={f.secret ? 'password' : 'text'}
                      value={row.values[f.key] ?? ''}
                      onChange={e => setRowValue(i, f.key, e.target.value)}
                      placeholder={f.optional ? `${f.label} (optional)` : f.label}
                      style={{ maxWidth: 220 }}
                      disabled={!activeApp}
                    />
                  ))}
                  <input
                    className="input"
                    data-testid={`provider-cred-weight-${i}`}
                    type="number" min={1}
                    value={row.weight}
                    onChange={e => setRowWeight(i, e.target.value)}
                    title="Weight (higher = more traffic)"
                    style={{ width: 84 }}
                    disabled={!activeApp}
                  />
                  <button
                    className="btn btn-danger"
                    data-testid={`provider-cred-remove-row-${i}`}
                    onClick={() => removeRow(i)}
                    disabled={!activeApp || credRows.length <= 1}
                    style={{ fontSize: 12, padding: '4px 10px' }}
                    title="Remove this key"
                  >×</button>
                </div>
              ))}
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <button className="btn" data-testid="provider-cred-add-row" onClick={addRow} disabled={!activeApp} style={{ fontSize: 13 }}>
                  + Add another {selectedProvider.pool_field === 'credential_sets' ? 'credential' : 'key'}
                </button>
                <button className="btn btn-primary" data-testid="provider-cred-add" onClick={addCred} disabled={!activeApp}>
                  Save credential
                </button>
              </div>
              <p style={{ color: '#6e7681', fontSize: 12, margin: 0 }}>
                {credRows.length > 1
                  ? 'Requests are load-balanced across these by weight (higher = more traffic).'
                  : 'Add more than one to load-balance requests across them by weight.'}
              </p>
            </>
          )}
        </div>
      </div>

      {/* Card 3 — App model access */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>App model access</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Which org models this app's API keys may call.
          {activeApp ? '' : ' — select an app to configure.'}
        </p>
        <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
          {(['all', 'select'] as const).map(m => (
            <button
              key={m}
              className={`btn${appModelMode === m ? ' btn-primary' : ''}`}
              onClick={() => { setAppModelMode(m); setAppModelDirty(true); }}
              disabled={!activeApp}
              style={{ minWidth: 100 }}
            >
              {m === 'all' ? 'All org models' : 'Select'}
            </button>
          ))}
        </div>
        {appModelMode === 'select' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginBottom: 16 }}>
            {[...orgModels].length === 0 ? (
              <div style={{ color: '#8b949e', fontSize: 14 }}>
                No models in org pool yet — add them above first.
              </div>
            ) : (
              [...orgModels].sort().map(modelId => (
                <label key={modelId} style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', padding: '6px 10px', borderRadius: 6 }}>
                  <input
                    type="checkbox"
                    checked={appModelIds.has(modelId)}
                    onChange={() => toggleAppModel(modelId)}
                    disabled={!activeApp}
                  />
                  <span style={{ color: '#e6edf3', fontSize: 14 }}>{modelId}</span>
                </label>
              ))
            )}
          </div>
        )}
        <button
          className="btn btn-primary"
          onClick={saveAppModels}
          disabled={!activeApp || !appModelDirty}
        >
          Save app model access
        </button>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Remove the provider credentials card from `Organization.tsx`**

Delete lines 312–443 (the entire `{/* Provider credentials */}` div block). The remaining file keeps: org name card, applications card, BYO S3 card. Also remove unused state/imports that were only used by the credentials section:

Remove these state declarations (lines ~28–34):
```tsx
const [creds, setCreds] = useState<ProviderCredentialInfo[]>([]);
const [catalog, setCatalog] = useState<ProviderCatalogEntry[]>([]);
const [credProvider, setCredProvider] = useState('');
type CredRow = { values: Record<string, string>; weight: string };
const emptyRow = (): CredRow => ({ values: {}, weight: '1' });
const [credRows, setCredRows] = useState<CredRow[]>([emptyRow()]);
```

Remove the `loadCreds` callback and its `useEffect` (lines ~108–121).

Remove the provider catalog `useEffect` (lines ~124–135).

Remove the `selectedProvider`, `changeProvider`, `setRowValue`, `setRowWeight`, `addRow`, `removeRow`, `addCred`, `deleteCred` declarations (lines ~171–227).

Remove the `ProviderCredentialInfo` and `ProviderCatalogEntry` imports from the type import line.

- [ ] **Step 3: Add Providers to nav and routing in `App.tsx`**

Add the import at the top:
```tsx
import Providers from './pages/Providers';
```

Add to `BASE_NAV_ITEMS` between Health and API Keys:
```tsx
{ to: '/providers', icon: Plug, label: 'Providers', testid: 'nav-providers' },
```

Add `Plug` to the lucide-react import:
```tsx
import { BarChart3, Activity, Layers, Database, Key, Plug, Sparkles, Film, ShieldAlert, Users as UsersIcon, Building2 } from 'lucide-react';
```

Add the route inside `<Routes>` alongside the others:
```tsx
<Route path="/providers" element={<Providers />} />
```

- [ ] **Step 4: Verify TypeScript compiles**

```bash
cd dashboard && npm run build 2>&1 | grep -E "error TS|Error"
```
Expected: no errors.

- [ ] **Step 5: Verify dev server renders correctly**

```bash
cd dashboard && npm run dev
```

Open http://localhost:5173/providers and verify:
- "Providers" appears in the sidebar between Health and API Keys
- Card 1 (Org model pool) shows the model catalog grouped by provider with checkboxes
- Card 2 (App credentials) shows existing creds and the add form (same as it was in Organization page)
- Card 3 (App model access) shows the All/Select toggle
- Organization page no longer has a Provider credentials card

- [ ] **Step 6: Commit**

```bash
git add dashboard/src/pages/Providers.tsx dashboard/src/pages/Organization.tsx dashboard/src/App.tsx
git commit -m "feat(dashboard): Providers page with org model pool and app model access"
```
