# Multi-Tenant Hosted Platform — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn LiteGen's single-tenant auth into a multi-tenant model — Organizations (with team members) → Applications → id/secret API keys — with full isolation, behind a backward-compatible `LITEGEN__MODE` switch, verified by real-HTTP integration tests and a DB-recreating Playwright e2e.

**Architecture:** Every data row gains `org_id` (+ `app_id` where applicable); a backfill migration moves existing rows into an auto-created "default" org/app so single-tenant deployments keep working. The auth middleware resolves a tenant context two ways — programmatic `Authorization: Bearer sk_live_…` (key row → org_id/app_id) and dashboard session cookie (`X-Litegen-Org-Id`/`X-Litegen-App-Id` headers validated against `organization_members`). Every query filters by the resolved tenant. `LITEGEN__MODE=single_tenant` (default) preserves today's master-key/closed-signup behavior scoped to the default org; `hosted` opens self-serve org-creating signup and demotes the master key to platform-admin (no implicit tenant data access).

**Tech Stack:** Rust (axum 0.8, sqlx, async-trait, sha2, argon2, uuid, chrono), SQLite + Postgres (mirrored migrations, `sqlx::migrate!` auto-runs on connect), `aes-gcm` (new dep) for BYO provider-cred encryption, React + Vite + `@litegen/sdk` (regenerated from OpenAPI), `reqwest`/`tempfile`/`wiremock` (integration tests), Playwright (e2e).

**Reference spec:** `docs/superpowers/specs/2026-06-03-multi-tenant-hosted-platform-design.md`

**Conventions confirmed from the codebase (do not deviate):**
- Migrations live in `litegen-core/migrations/{postgres,sqlite}/` as `<timestamp>_<name>.sql`; both backends auto-run `sqlx::migrate!` on `connect`. **Every schema change is two mirrored files.** Next number: `20240101000008_*`.
- The axum app is built by `litegen::api::create_router(state: Arc<AppState>) -> Router`. `AppState` is a plain struct literal (8 fields, no `::new`).
- `Role` uses `Role::parse(&str) -> Option<Role>` and `Role::as_str()`. Permissions are an enum in `auth/permissions.rs` with `as_str()` + `permissions_for(role) -> &'static [Permission]`.
- IDs: `users.id`, `sessions.id`, `invitations.id` are `String`; `api_keys.id` is `Uuid`; `generations.id` is `String`. New tenant tables use `String` (UUID v4 text) ids for consistency with users.
- DB trait methods that mock impls don't need have **default impls** in `trait_def.rs` (so the mock `DatabaseStore` in tests still compiles). Follow that pattern for every new method.
- Config: prefix `LITEGEN`, separator `__` (so `LITEGEN__MODE` → `config.mode`). CORS + models dir use single-underscore special vars.

---

## File Structure (created / modified in Phase 1)

**Created**
- `litegen-core/migrations/postgres/20240101000008_multitenant.sql` + `…/sqlite/20240101000008_multitenant.sql` — tenancy schema + backfill.
- `litegen-core/src/auth/secrets.rs` — AES-256-GCM encrypt/decrypt for BYO provider creds; key-id/secret generation (`pk_live_`/`sk_live_`).
- `litegen-core/src/api/handlers/orgs.rs` — org/app/member/provider-cred endpoints.
- `litegen-core/tests/multitenant_api.rs` — real-HTTP integration suite + `spawn_app()` harness.
- `dashboard/src/context/TenantContext.tsx` — active org/app provider + switcher state.
- `dashboard/src/components/OrgSwitcher.tsx`, `dashboard/src/pages/Organization.tsx`, `dashboard/src/pages/Members.tsx`, `dashboard/src/pages/Apps.tsx`.
- `dashboard/e2e/multitenant.spec.ts` — multi-tenant UI flow + isolation.

**Modified**
- `litegen-core/Cargo.toml` — add `aes-gcm`, `base64` (if absent), `rand` features.
- `litegen-core/src/config/mod.rs` — `mode`, `secrets_key`, dev flags.
- `litegen-core/src/types/mod.rs` — `Organization`, `Application`, `OrganizationMember`, `ProviderCredentialInfo`; extend `ApiKey`/`ApiKeyCreatedResponse`/`ApiKeyInfo`.
- `litegen-core/src/db/trait_def.rs`, `db/sqlite.rs`, `db/postgres.rs` — tenancy methods + tenant-scoped query variants.
- `litegen-core/src/auth/permissions.rs` — org/app/member/provider_cred permissions.
- `litegen-core/src/api/middleware/mod.rs` — `KeyContext.{org_id,app_id}`, tenant resolution, mode hardening, `AppState.{mode,secrets_key}`.
- `litegen-core/src/api/handlers/mod.rs` — key create/list scoped to app (id/secret), signup→org, generate/logs/stats scoping, route registration.
- `litegen-core/src/api/handlers/auth_password.rs` — signup creates org.
- `litegen-core/src/api/openapi.rs` — register new schemas/paths.
- `litegen-core/src/main.rs` — wire `mode`/`secrets_key` into `AppState`.
- `dashboard/src/sdk-client.ts` — inject `X-Litegen-Org-Id`/`X-Litegen-App-Id`.
- `dashboard/src/App.tsx`, `pages/Signup.tsx`, `pages/Keys.tsx`, `components/UserMenu.tsx` — tenant context + onboarding + scoped views.
- `dashboard/playwright.config.ts`, `dashboard/vite.config.ts` — recreate-DB-each-run + serve dashboard.
- SDK: regenerated `sdks/typescript` + `sdks/python` via `sdks/scripts/regen-all.sh`.

---

## Task 1: Config — mode switch, secrets key, dev flags

**Files:**
- Modify: `litegen-core/src/config/mod.rs`
- Modify: `litegen-core/src/api/middleware/mod.rs` (`AppState`)
- Modify: `litegen-core/src/main.rs`
- Test: `litegen-core/src/config/mod.rs` (`#[cfg(test)] mod`)

- [ ] **Step 1: Write the failing test** (append to the config test module)

```rust
#[test]
fn mode_defaults_to_single_tenant_and_parses_env() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.mode, Mode::SingleTenant);
    // Hosted parses from the lowercase string used by the LITEGEN__MODE env var.
    assert_eq!(Mode::parse("hosted"), Some(Mode::Hosted));
    assert_eq!(Mode::parse("single_tenant"), Some(Mode::SingleTenant));
}
```

- [ ] **Step 2: Run it, expect FAIL** — `cd litegen-core && cargo test --lib config::tests::mode_defaults_to_single_tenant_and_parses_env` → fails (`Mode` undefined).

- [ ] **Step 3: Implement.** Add to `config/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode { SingleTenant, Hosted }

impl Default for Mode { fn default() -> Self { Mode::SingleTenant } }
impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "single_tenant" | "single" => Some(Mode::SingleTenant),
            "hosted" | "multi_tenant" => Some(Mode::Hosted),
            _ => None,
        }
    }
}
```

Add fields to `AppConfig` (find the struct; add with `#[serde(default)]`):

```rust
    #[serde(default)]
    pub mode: Mode,
    /// Base64 32-byte key for encrypting BYO provider credentials at rest. Required in hosted mode.
    #[serde(default)]
    pub secrets_key: Option<String>,
    #[serde(default)]
    pub dev: DevFlags,
```

```rust
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DevFlags {
    #[serde(default)] pub expose_invite_tokens: bool,
    #[serde(default)] pub expose_reset_tokens: bool,
}
```

- [ ] **Step 4: Run test, expect PASS.**

- [ ] **Step 5: Thread into `AppState`.** In `middleware/mod.rs` add to `AppState`: `pub mode: crate::config::Mode,` and `pub secrets_key: Option<Vec<u8>>,` and `pub dev: crate::config::DevFlags,`. In `main.rs` where `AppState { … }` is built, set `mode: config.mode`, `dev: config.dev.clone()`, and `secrets_key: config.secrets_key.as_deref().map(|s| base64_decode_32(s)).transpose()…` (decode + validate 32 bytes; on hosted mode with no/invalid key, log a fatal error and exit). Fix the two test `build_test_state()` sites (handlers + middleware test mods) to include the new fields (`mode: Mode::SingleTenant, secrets_key: None, dev: DevFlags::default()`).

- [ ] **Step 6: Commit** — `git commit -am "feat(config): add LITEGEN__MODE, secrets_key, dev flags"`

---

## Task 2: Migrations — tenancy schema + backfill (postgres + sqlite mirrors)

**Files:**
- Create: `litegen-core/migrations/sqlite/20240101000008_multitenant.sql`
- Create: `litegen-core/migrations/postgres/20240101000008_multitenant.sql`
- Test: `litegen-core/tests/multitenant_api.rs` (a `schema_has_tenant_tables` test, created here, fleshed out in Task 11)

- [ ] **Step 1: Write the SQLite migration** (`migrations/sqlite/20240101000008_multitenant.sql`). Use `TEXT`/`INTEGER`/`TIMESTAMP` to match existing sqlite migrations.

```sql
-- Organizations (tenants)
CREATE TABLE organizations (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    slug        TEXT NOT NULL UNIQUE,
    plan        TEXT NOT NULL DEFAULT 'free',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE organization_members (
    org_id      TEXT NOT NULL REFERENCES organizations(id),
    user_id     TEXT NOT NULL REFERENCES users(id),
    role        TEXT NOT NULL CHECK (role IN ('owner','admin','member','viewer')),
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (org_id, user_id)
);
CREATE INDEX idx_org_members_user ON organization_members(user_id);

CREATE TABLE applications (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL REFERENCES organizations(id),
    name        TEXT NOT NULL,
    slug        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (org_id, slug)
);
CREATE INDEX idx_applications_org ON applications(org_id);

CREATE TABLE provider_credentials (
    id            TEXT PRIMARY KEY,
    app_id        TEXT NOT NULL REFERENCES applications(id),
    provider      TEXT NOT NULL,
    ciphertext    TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    display_hint  TEXT,
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (app_id, provider)
);

-- Tenant columns on existing tables (nullable add; backfilled below; kept nullable
-- because SQLite cannot add a NOT NULL column without a default to an existing table).
ALTER TABLE api_keys           ADD COLUMN org_id TEXT REFERENCES organizations(id);
ALTER TABLE api_keys           ADD COLUMN app_id TEXT REFERENCES applications(id);
ALTER TABLE api_keys           ADD COLUMN public_id TEXT;
ALTER TABLE generations        ADD COLUMN org_id TEXT;
ALTER TABLE generations        ADD COLUMN app_id TEXT;
ALTER TABLE request_logs       ADD COLUMN org_id TEXT;
ALTER TABLE request_logs       ADD COLUMN app_id TEXT;
ALTER TABLE request_artifacts  ADD COLUMN org_id TEXT;
ALTER TABLE request_artifacts  ADD COLUMN app_id TEXT;
ALTER TABLE webhook_deliveries ADD COLUMN org_id TEXT;
ALTER TABLE webhook_deliveries ADD COLUMN app_id TEXT;
ALTER TABLE audit_log          ADD COLUMN org_id TEXT;
ALTER TABLE invitations        ADD COLUMN org_id TEXT;

-- Backfill: one default org + app, members from existing users, stamp existing rows.
INSERT INTO organizations (id, name, slug) VALUES ('00000000-0000-0000-0000-000000000001', 'Default', 'default');
INSERT INTO applications (id, org_id, name, slug)
    VALUES ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000001', 'Default', 'default');
INSERT INTO organization_members (org_id, user_id, role)
    SELECT '00000000-0000-0000-0000-000000000001', id, role FROM users;
UPDATE api_keys           SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE generations        SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE request_logs       SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE request_artifacts  SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE webhook_deliveries SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE audit_log          SET org_id = '00000000-0000-0000-0000-000000000001' WHERE org_id IS NULL;
UPDATE invitations        SET org_id = '00000000-0000-0000-0000-000000000001' WHERE org_id IS NULL;

CREATE INDEX idx_api_keys_tenant     ON api_keys(org_id, app_id, created_at);
CREATE INDEX idx_generations_tenant  ON generations(org_id, app_id, created_at);
CREATE INDEX idx_request_logs_tenant ON request_logs(org_id, app_id, created_at);
CREATE INDEX idx_audit_log_tenant    ON audit_log(org_id, created_at);
CREATE UNIQUE INDEX idx_api_keys_public_id ON api_keys(public_id) WHERE public_id IS NOT NULL;
```

- [ ] **Step 2: Write the Postgres migration** — same statements, with Postgres types: `TIMESTAMPTZ` instead of `TIMESTAMP`, `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`, and Postgres allows multiple `ADD COLUMN` in one `ALTER TABLE` (combine per table). Partial unique index syntax is identical. Keep the same fixed default UUIDs.

- [ ] **Step 3: Verify migrations apply on a fresh DB.** Run `cd litegen-core && cargo test --lib db::sqlite` (existing sqlite tests connect a temp DB and migrate) → expect PASS (proves the new migration parses + applies). If a statement errors, fix the SQL. Also: `DATABASE_URL=sqlite::memory: cargo sqlx` is not used here — migrations are embedded, so a passing build + sqlite test is the gate.

- [ ] **Step 4: Commit** — `git commit -am "feat(db): multitenant schema (orgs/apps/members/provider_credentials) + backfill migration"`

---

## Task 3: Types — org/app/member structs; extend key types

**Files:**
- Modify: `litegen-core/src/types/mod.rs`
- Test: `litegen-core/src/types/mod.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn org_and_app_serde_roundtrip() {
    let org = Organization { id: "o1".into(), name: "Acme".into(), slug: "acme".into(),
        plan: "free".into(), status: "active".into(),
        created_at: chrono::Utc::now(), updated_at: chrono::Utc::now() };
    let j = serde_json::to_string(&org).unwrap();
    let back: Organization = serde_json::from_str(&j).unwrap();
    assert_eq!(back.slug, "acme");
}
```

- [ ] **Step 2: Run, expect FAIL** (`Organization` undefined).

- [ ] **Step 3: Implement.** Add to `types/mod.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Organization {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Application {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrganizationMember {
    pub org_id: String,
    pub user_id: String,
    pub email: String,          // joined from users for list views
    pub role: Role,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Public view of a stored BYO provider credential — NEVER the plaintext secret.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderCredentialInfo {
    pub provider: String,
    pub display_hint: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 4: Extend `ApiKey`** — add three fields (after `owner_user_id`):

```rust
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    /// Public key id shown to customers, e.g. "pk_live_…". None for legacy lg- keys.
    pub public_id: Option<String>,
```

Extend `ApiKeyCreatedResponse` to add `pub public_id: String,` and `pub id: Uuid,`. Extend `ApiKeyInfo` to add `pub public_id: Option<String>,` and `pub app_id: Option<String>,`. Update the `From<ApiKey>`/construction sites that build these (compiler will point them out) to populate the new fields.

- [ ] **Step 5: Run** `cargo test --lib types::tests::org_and_app_serde_roundtrip` → PASS, and `cargo build --lib` → fix any construction sites flagged.

- [ ] **Step 6: Commit** — `git commit -am "feat(types): org/app/member/provider-cred types; id+secret key fields"`

---

## Task 4: DB trait + SQLite + Postgres tenancy methods

**Files:**
- Modify: `litegen-core/src/db/trait_def.rs` (new methods, all with default impls)
- Modify: `litegen-core/src/db/sqlite.rs`, `litegen-core/src/db/postgres.rs` (real impls)
- Test: `litegen-core/src/db/sqlite_tests.rs`

**Pattern:** add each method to the trait with a default impl returning empty/`Ok(())` (so the test-mock `DatabaseStore` still compiles), then implement for real in both `sqlite.rs` and `postgres.rs`. SQLite uses `?` placeholders + `query_as`/`query`; Postgres uses `$1` placeholders. Mirror exactly.

- [ ] **Step 1: Add trait methods** to `trait_def.rs` (default impls). Group under a `// ─── Tenancy ───` header:

```rust
    // Organizations
    async fn create_organization(&self, _o: &Organization) -> Result<(), sqlx::Error> { Ok(()) }
    async fn get_organization(&self, _id: &str) -> Result<Option<Organization>, sqlx::Error> { Ok(None) }
    async fn get_org_by_slug(&self, _slug: &str) -> Result<Option<Organization>, sqlx::Error> { Ok(None) }
    async fn list_orgs_for_user(&self, _user_id: &str) -> Result<Vec<(Organization, Role)>, sqlx::Error> { Ok(vec![]) }
    async fn update_organization(&self, _id: &str, _name: Option<&str>) -> Result<Option<Organization>, sqlx::Error> { Ok(None) }
    async fn delete_organization(&self, _id: &str) -> Result<bool, sqlx::Error> { Ok(false) }
    // Members
    async fn add_org_member(&self, _org_id: &str, _user_id: &str, _role: Role) -> Result<(), sqlx::Error> { Ok(()) }
    async fn get_membership(&self, _org_id: &str, _user_id: &str) -> Result<Option<Role>, sqlx::Error> { Ok(None) }
    async fn list_org_members(&self, _org_id: &str) -> Result<Vec<OrganizationMember>, sqlx::Error> { Ok(vec![]) }
    async fn update_member_role(&self, _org_id: &str, _user_id: &str, _role: Role) -> Result<(), sqlx::Error> { Ok(()) }
    async fn remove_org_member(&self, _org_id: &str, _user_id: &str) -> Result<(), sqlx::Error> { Ok(()) }
    async fn transfer_org_owner(&self, _org_id: &str, _new_owner_user_id: &str) -> Result<(), sqlx::Error> { Ok(()) }
    // Applications
    async fn create_application(&self, _a: &Application) -> Result<(), sqlx::Error> { Ok(()) }
    async fn get_application(&self, _id: &str) -> Result<Option<Application>, sqlx::Error> { Ok(None) }
    async fn list_apps_for_org(&self, _org_id: &str) -> Result<Vec<Application>, sqlx::Error> { Ok(vec![]) }
    async fn update_application(&self, _id: &str, _name: Option<&str>) -> Result<Option<Application>, sqlx::Error> { Ok(None) }
    async fn delete_application(&self, _id: &str) -> Result<bool, sqlx::Error> { Ok(false) }
    // Provider credentials (BYO) — store ciphertext; plaintext is never persisted in cleartext
    async fn upsert_provider_credential(&self, _app_id: &str, _provider: &str, _ciphertext: &str, _nonce: &str, _display_hint: Option<&str>) -> Result<(), sqlx::Error> { Ok(()) }
    async fn get_provider_credential(&self, _app_id: &str, _provider: &str) -> Result<Option<(String, String)>, sqlx::Error> { Ok(None) } // (ciphertext, nonce)
    async fn list_provider_credentials(&self, _app_id: &str) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> { Ok(vec![]) }
    async fn delete_provider_credential(&self, _app_id: &str, _provider: &str) -> Result<bool, sqlx::Error> { Ok(false) }
```

- [ ] **Step 2: Change tenant-aware signatures** (breaking — the compiler enforces every call site is updated):
  - `create_api_key(...)` — add params `org_id: &str, app_id: &str, public_id: &str` (before `name`). Persist `org_id`, `app_id`, `public_id` columns.
  - `list_api_keys_for_app(&self, app_id: &str) -> Result<Vec<ApiKey>, sqlx::Error>` (new) — used by the scoped list endpoint.
  - `list_generations(&self, org_id: &str, app_id: Option<&str>, page, per_page)` and `count_generations(&self, org_id: &str, app_id: Option<&str>)` — replace the `key_id` filter with an `org_id` (+ optional `app_id`) filter. **Update the doc comment** (currently describes the master-key NULL behavior) to describe tenant filtering.
  - `get_request_logs(&self, org_id: &str, app_id: Option<&str>, page, per_page)` and `get_request_logs_filtered(...)` — add `org_id`/`app_id` filter.
  - `get_stats(&self, org_id: &str, app_id: Option<&str>)` and `list_audit_log(filter: &AuditLogFilter, ...)` where `AuditLogFilter` gains an `org_id: Option<String>` field.
  - `log_request(...)` and `insert_generation(...)` and `insert_request_artifact(...)` and `insert_webhook_delivery(...)` — add `org_id: Option<&str>, app_id: Option<&str>` params and write them.

- [ ] **Step 3: Implement in `sqlite.rs`.** Example — the canonical tenant-scoped read (apply the same shape to logs/stats/audit):

```rust
async fn list_generations(&self, org_id: &str, app_id: Option<&str>, page: u32, per_page: u32)
    -> Result<Vec<Generation>, sqlx::Error>
{
    let offset = (page.saturating_sub(1) * per_page) as i64;
    let rows = match app_id {
        Some(app) => sqlx::query_as::<_, GenerationRow>(
            "SELECT * FROM generations WHERE org_id = ? AND app_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?")
            .bind(org_id).bind(app).bind(per_page as i64).bind(offset).fetch_all(&self.pool).await?,
        None => sqlx::query_as::<_, GenerationRow>(
            "SELECT * FROM generations WHERE org_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?")
            .bind(org_id).bind(per_page as i64).bind(offset).fetch_all(&self.pool).await?,
    };
    Ok(rows.into_iter().map(Into::into).collect())
}
```

And the canonical write (`create_api_key`) — generate nothing here (the handler makes ids/hashes); just persist:

```rust
async fn create_api_key(&self, org_id: &str, app_id: &str, public_id: &str, name: &str,
    key_hash: &str, key_prefix: &str, token_quota: Option<f64>, rpm_limit: Option<u32>,
    scopes: &str, webhook_url: Option<&str>) -> Result<ApiKey, sqlx::Error>
{
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys (id, org_id, app_id, public_id, name, key_hash, key_prefix, is_active, tokens_used, token_quota, rpm_limit, scopes, webhook_url, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 1, 0, ?, ?, ?, ?, CURRENT_TIMESTAMP)")
        .bind(id.to_string()).bind(org_id).bind(app_id).bind(public_id).bind(name)
        .bind(key_hash).bind(key_prefix).bind(token_quota).bind(rpm_limit).bind(scopes).bind(webhook_url)
        .execute(&self.pool).await?;
    self.get_api_key(&id).await?.ok_or(sqlx::Error::RowNotFound)
}
```

Implement all org/app/member/provider-cred methods. Org owner transfer within a single org:

```rust
async fn transfer_org_owner(&self, org_id: &str, new_owner_user_id: &str) -> Result<(), sqlx::Error> {
    let mut tx = self.pool.begin().await?;
    sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = ? AND role = 'owner'")
        .bind(org_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE organization_members SET role = 'owner' WHERE org_id = ? AND user_id = ?")
        .bind(org_id).bind(new_owner_user_id).execute(&mut *tx).await?;
    tx.commit().await
}
```

`get_membership` returns the role:

```rust
async fn get_membership(&self, org_id: &str, user_id: &str) -> Result<Option<Role>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM organization_members WHERE org_id = ? AND user_id = ?")
        .bind(org_id).bind(user_id).fetch_optional(&self.pool).await?;
    Ok(row.and_then(|(r,)| Role::parse(&r)))
}
```

- [ ] **Step 4: Mirror in `postgres.rs`** — identical logic, `$1..$n` placeholders, `now()` for timestamps, `LIMIT $x OFFSET $y`. Wherever the existing postgres impl maps rows, follow that pattern.

- [ ] **Step 5: Update all broken call sites.** `cargo build --lib` will list every place that called the changed signatures (`create_api_key`, `list_generations`, `count_generations`, `get_request_logs*`, `get_stats`, `log_request`, `insert_generation`, etc.). They are fixed contextually in Tasks 5–9; for now, get the trait + impls compiling by leaving the handler call sites to those tasks (compile `--lib` may stay red until Task 9 — that's expected; commit the DB layer with its own unit tests passing via the mock/sqlite paths).

- [ ] **Step 6: DB unit tests** in `sqlite_tests.rs`:

```rust
#[tokio::test]
async fn org_app_member_crud_and_isolation() {
    let db = test_db().await; // existing helper: connects a temp sqlite + migrates
    let org_a = mk_org(&db, "a").await; let app_a = mk_app(&db, &org_a, "prod").await;
    let org_b = mk_org(&db, "b").await; let app_b = mk_app(&db, &org_b, "prod").await;
    // a key + a generation in each org
    db.create_api_key(&org_a, &app_a, "pk_live_a", "k", "hashA", "sk_live_aaa", None, None, "generate,read", None).await.unwrap();
    db.insert_generation("g-a", None, "mock/img", "mock", "image", None, 0.0 /* + org_a, app_a via new params */).await.unwrap();
    db.insert_generation("g-b", None, "mock/img", "mock", "image", None, 0.0 /* + org_b, app_b */).await.unwrap();
    let a_gens = db.list_generations(&org_a, Some(&app_a), 1, 50).await.unwrap();
    let b_gens = db.list_generations(&org_b, Some(&app_b), 1, 50).await.unwrap();
    assert_eq!(a_gens.len(), 1); assert_eq!(b_gens.len(), 1);
    assert!(a_gens.iter().all(|g| g.id != "g-b")); // org A never sees org B's rows
}
```

(Provide `mk_org`/`mk_app` tiny helpers in the test module.)

- [ ] **Step 7: Run** `cargo test --lib db::sqlite_tests::org_app_member_crud_and_isolation` → PASS.

- [ ] **Step 8: Commit** — `git commit -am "feat(db): tenancy CRUD + tenant-scoped generation/log/stats/audit queries"`

---

## Task 5: id/secret API keys + `secrets.rs`

**Files:**
- Create: `litegen-core/src/auth/secrets.rs` (key id/secret + AES-GCM)
- Modify: `litegen-core/Cargo.toml` (add `aes-gcm = "0.10"`, ensure `base64`, `rand`)
- Modify: `litegen-core/src/auth/mod.rs` (`pub mod secrets;`)
- Test: `litegen-core/src/auth/secrets.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Add deps** to `Cargo.toml` `[dependencies]`: `aes-gcm = "0.10"`, and confirm `base64` + `rand` exist (add `base64 = "0.22"` if missing). Run `cargo build --lib` to fetch.

- [ ] **Step 2: Write failing tests** in `secrets.rs`:

```rust
#[test]
fn generates_prefixed_keypair() {
    let kp = generate_key_pair();
    assert!(kp.public_id.starts_with("pk_live_"));
    assert!(kp.secret.starts_with("sk_live_"));
    assert_ne!(kp.public_id, kp.secret);
    assert_eq!(kp.secret_hash, sha256_hex(&kp.secret));
}

#[test]
fn aes_roundtrip() {
    let key = [7u8; 32];
    let (ct, nonce) = encrypt(&key, b"super-secret-openai-key").unwrap();
    let pt = decrypt(&key, &ct, &nonce).unwrap();
    assert_eq!(pt, b"super-secret-openai-key");
    // wrong key fails
    assert!(decrypt(&[9u8;32], &ct, &nonce).is_err());
}
```

- [ ] **Step 3: Run, expect FAIL.**

- [ ] **Step 4: Implement** `secrets.rs`:

```rust
use aes_gcm::{aead::{Aead, KeyInit, OsRng}, Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub struct KeyPair { pub public_id: String, pub secret: String, pub secret_hash: String, pub prefix: String }

fn token(prefix: &str, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    format!("{prefix}{}", hex::encode(buf)) // or base62; hex is fine and url-safe
}

pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new(); h.update(s.as_bytes()); hex::encode(h.finalize())
}

pub fn generate_key_pair() -> KeyPair {
    let public_id = token("pk_live_", 12);
    let secret = token("sk_live_", 24);
    KeyPair { public_id, prefix: secret[..16.min(secret.len())].to_string(),
              secret_hash: sha256_hex(&secret), secret }
}

/// Returns (base64 ciphertext, base64 nonce).
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<(String, String), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher.encrypt(nonce, plaintext).map_err(|e| e.to_string())?;
    Ok((STANDARD.encode(ct), STANDARD.encode(nonce_bytes)))
}

pub fn decrypt(key: &[u8; 32], ct_b64: &str, nonce_b64: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let cipher = Aes256Gcm::new(key.into());
    let ct = STANDARD.decode(ct_b64).map_err(|e| e.to_string())?;
    let nonce_bytes = STANDARD.decode(nonce_b64).map_err(|e| e.to_string())?;
    cipher.decrypt(Nonce::from_slice(&nonce_bytes), ct.as_ref()).map_err(|e| e.to_string())
}
```

(If `hex` isn't a dep, add `hex = "0.4"`.)

- [ ] **Step 5: Run** `cargo test --lib auth::secrets` → PASS.

- [ ] **Step 6: Rewrite `create_api_key` handler** (`handlers/mod.rs`) to use the pair + active app from `KeyContext`:

```rust
let kp = crate::auth::secrets::generate_key_pair();
let org_id = ctx.org_id.clone().ok_or(/* 400 no active org */)?;
let app_id = ctx.app_id.clone().ok_or(/* 400 no active app */)?;
let key = state.db.create_api_key(&org_id, &app_id, &kp.public_id, &req.name,
    &kp.secret_hash, &kp.prefix, req.token_quota, req.rpm_limit, &req.scopes, req.webhook_url.as_deref()).await?;
// owner_user_id still set from session user if present (existing set_api_key_owner call)
let resp = ApiKeyCreatedResponse { id: key.id, key: kp.secret, public_id: kp.public_id,
    prefix: kp.prefix, name: key.name, created_at: key.created_at,
    token_quota: key.token_quota, rpm_limit: key.rpm_limit, scopes: key.scopes };
```

Lookup is unchanged: middleware already SHA-256s the bearer and calls `lookup_api_key_by_hash` — old `lg-` keys keep working because their hash is still in `key_hash`.

- [ ] **Step 7: Integration test** (added to `tests/multitenant_api.rs`, harness from Task 11): create key → assert `public_id` starts `pk_live_`, `key` starts `sk_live_`; then `Authorization: Bearer <key>` on `POST /v1/images/generations` with `{"model":"mock/...","prompt":"x"}` → 200.

- [ ] **Step 8: Commit** — `git commit -am "feat(auth): id+secret API keys (pk_live_/sk_live_) + AES-GCM secrets module"`

---

## Task 6: Tenant-context middleware + mode hardening

**Files:**
- Modify: `litegen-core/src/api/middleware/mod.rs`
- Test: `litegen-core/src/api/middleware/auth_tests.rs` + `tests/multitenant_api.rs`

- [ ] **Step 1: Extend `KeyContext`** — add `pub org_id: Option<String>,` and `pub app_id: Option<String>,`. Fix every `KeyContext { … }` literal in this file (there are ~5) to set the new fields (`org_id: None, app_id: None` for the non-tenant ones initially).

- [ ] **Step 2: Bearer DB-key path** (`handle_db_key`) — after loading the key, set `org_id: key.org_id.clone(), app_id: key.app_id.clone()` on the context.

- [ ] **Step 3: Session path** — after loading `user`, resolve the active org/app:

```rust
// active org: header if a member, else the user's first org
let header_org = request.headers().get("x-litegen-org-id").and_then(|v| v.to_str().ok()).map(str::to_string);
let (org_id, role) = match header_org {
    Some(o) => match state.db.get_membership(&o, &user.id).await {
        Ok(Some(r)) => (Some(o), r),
        _ => return forbidden_response("Not a member of the requested organization"),
    },
    None => match state.db.list_orgs_for_user(&user.id).await { Ok(mut v) if !v.is_empty() => {
        let (org, r) = v.remove(0); (Some(org.id), r) } _ => (None, user.role) },
};
// active app: header validated to belong to org, else first app of the org
let header_app = request.headers().get("x-litegen-app-id").and_then(|v| v.to_str().ok()).map(str::to_string);
let app_id = match (&org_id, header_app) {
    (Some(o), Some(a)) => match state.db.get_application(&a).await {
        Ok(Some(app)) if &app.org_id == o => Some(a),
        _ => return forbidden_response("Application does not belong to the active organization"),
    },
    (Some(o), None) => state.db.list_apps_for_org(o).await.ok().and_then(|mut v| if v.is_empty(){None}else{Some(v.remove(0).id)}),
    _ => None,
};
let perms = permissions_for(role).to_vec(); // role is now the MEMBERSHIP role
```

Set `org_id`, `app_id`, `permissions: perms` on the session `KeyContext`. (Add a `forbidden_response(msg)` helper mirroring `unauthorized_response`, returning 403 `forbidden`.)

- [ ] **Step 4: Mode hardening.** Guard the two dev bypass branches (`state.master_key.is_none()`):

```rust
if state.master_key.is_none() && state.mode == crate::config::Mode::SingleTenant {
    // existing dev all-scopes context, but stamp the default org/app
    // (look them up once at startup or by slug 'default')
}
// hosted mode: no master key + no auth => fall through to 401
```

For the master-key match branch: in `SingleTenant` keep all-scopes and set `org_id`/`app_id` to the default org/app (resolve `get_org_by_slug("default")` + its first app, cached). In `Hosted`, leave `org_id: None` (platform admin) — tenant-scoped handlers must reject `org_id: None` with 403, and only `/v1/admin/*` accepts a null-org master context.

- [ ] **Step 5: Failing integration test** (`tests/multitenant_api.rs`): `cross_tenant_isolation` — sign up org A + org B (two cookie jars), create a key in A, then with B's session send `X-Litegen-Org-Id: <A>` to `GET /v1/keys` → expect `403`; B listing its own keys → does not include A's. (Full body in Task 11.)

- [ ] **Step 6: Run** the test → PASS after Steps 1–4. Also run existing `cargo test --lib api::middleware::auth_tests` (single-tenant master-key path) → still PASS.

- [ ] **Step 7: Commit** — `git commit -am "feat(auth): tenant-context resolution in middleware + mode-based master-key/dev hardening"`

---

## Task 7: Signup → organization creation (hosted) + `/v1/auth/me`

**Files:**
- Modify: `litegen-core/src/api/handlers/auth_password.rs` (signup)
- Modify: `litegen-core/src/api/handlers/account.rs` or wherever `/v1/auth/me` lives
- Test: `tests/multitenant_api.rs`

- [ ] **Step 1: Failing test** — hosted signup with `{email,password,org_name:"Acme"}` returns 200 + session cookie; `GET /v1/orgs` (Task 8) lists one org named "Acme"; `GET /v1/auth/me` includes `active_org`.

- [ ] **Step 2: Implement signup branch.** In `signup`, branch on `state.mode`:
  - `SingleTenant`: keep the existing `count_users()>0 → 409` gate + `LITEGEN__OWNER_EMAIL`; on first signup also `add_org_member(default_org, user.id, Owner)`.
  - `Hosted`: do NOT gate on `count_users()`. Inside a transaction-like sequence: create user (Argon2 hash, platform role `user`), `create_organization`(name = `org_name` or derived from email, unique slug via slugify + dedup), `add_org_member(org.id, user.id, Owner)`, `create_application(org.id, "Default", "default")`, create session, set cookie. Return `{user, org}`.

```rust
let org = Organization { id: uuid::Uuid::new_v4().to_string(), name: org_name.clone(),
    slug: unique_slug(&state.db, &slugify(&org_name)).await?, plan: "free".into(),
    status: "active".into(), created_at: now, updated_at: now };
state.db.create_organization(&org).await?;
state.db.add_org_member(&org.id, &user.id, Role::Owner).await?;
let app = Application { id: uuid::Uuid::new_v4().to_string(), org_id: org.id.clone(),
    name: "Default".into(), slug: "default".into(), status: "active".into(),
    created_at: now, updated_at: now };
state.db.create_application(&app).await?;
```

- [ ] **Step 3: Extend `/v1/auth/me`** to return `{ user, orgs: [{id,name,role}], active_org, active_app }` (read `ctx.org_id`/`ctx.app_id` + `list_orgs_for_user`).

- [ ] **Step 4: Run test → PASS.**

- [ ] **Step 5: Commit** — `git commit -am "feat(auth): hosted signup creates org+owner+first app; /auth/me returns tenant context"`

---

## Task 8: Org / App / Member / Invitation endpoints + per-org permissions

**Files:**
- Create: `litegen-core/src/api/handlers/orgs.rs`
- Modify: `litegen-core/src/auth/permissions.rs`, `handlers/mod.rs` (route registration), `handlers/users.rs` (org-scope invites)
- Test: `tests/multitenant_api.rs`

- [ ] **Step 1: Add permissions.** In `permissions.rs` add enum variants `OrgRead, OrgWrite, OrgDelete, OrgTransferOwner, AppRead, AppWrite, AppDelete, MemberRead, MemberInvite, MemberWrite, MemberRemove, ProviderCredRead, ProviderCredWrite, ProviderCredDelete`, their `as_str()` arms (`"org:read"` …), and add them to `permissions_for`: Owner gets all; Admin gets all except `OrgDelete`+`OrgTransferOwner`; Member gets `OrgRead, AppRead, AppWrite, MemberRead, ProviderCredRead, ProviderCredWrite`; Viewer gets `OrgRead, AppRead, MemberRead`. Add a unit test mirroring the existing ones (e.g. `member_cannot_delete_org`).

- [ ] **Step 2: Implement handlers** in `orgs.rs`. Each reads `Extension<KeyContext>`, checks the membership permission for `ctx.org_id`, and calls the DB. Endpoints:
  - `POST /v1/orgs` (any authed user) → create org + owner membership + default app.
  - `GET /v1/orgs` → `list_orgs_for_user(ctx.user.id)`.
  - `GET/PATCH/DELETE /v1/orgs/{id}` → require `OrgRead`/`OrgWrite`/`OrgDelete` **and** membership in `{id}`.
  - `GET/POST /v1/orgs/{id}/members`, `PATCH/DELETE /v1/orgs/{id}/members/{user_id}`, `POST /v1/orgs/{id}/transfer-owner`.
  - `GET/POST /v1/orgs/{id}/apps`, `GET/PATCH/DELETE /v1/apps/{app_id}` (verify the app's org matches a membership).
  - `GET/POST /v1/apps/{app_id}/provider-credentials`, `DELETE /v1/apps/{app_id}/provider-credentials/{provider}` — POST encrypts with `state.secrets_key` (400 if hosted and key missing), stores ciphertext+nonce+display_hint (e.g. `…last4`); GET returns `ProviderCredentialInfo` (never plaintext).

A representative handler:

```rust
pub async fn list_org_members(State(state): State<Arc<AppState>>, Extension(ctx): Extension<KeyContext>,
    Path(org_id): Path<String>) -> Result<Json<Vec<OrganizationMember>>, ApiError> {
    require_member_perm(&state, &ctx, &org_id, Permission::MemberRead).await?;
    Ok(Json(state.db.list_org_members(&org_id).await.map_err(internal)?))
}
```

Add `require_member_perm(state, ctx, org_id, perm)` helper: fetch `get_membership(org_id, user_id)`, return 403 unless the role has `perm` (use `role_has`), and 403 if `ctx.user` is None (Bearer keys cannot manage orgs).

- [ ] **Step 3: Org-scope invitations.** In `users.rs` `invite_user`/`accept_invitation`: invitations now carry `org_id` (the inviter's active org); accept adds an `organization_members(org_id, new_user, invited_role)` row (create the user first if new). Keep the `LITEGEN__DEV__EXPOSE_INVITE_TOKENS` behavior (now reads `state.dev.expose_invite_tokens`).

- [ ] **Step 4: Register routes** in `create_router` (`handlers/mod.rs`) under the session/permission guards, alongside the existing `/v1/users` block. Add `#[utoipa::path]` to each handler.

- [ ] **Step 5: Tests** — invitation+role matrix in `tests/multitenant_api.rs`: owner invites member → accept → member can `GET /v1/orgs/{id}/apps` but `POST /v1/orgs/{id}/members` → 403; only owner `transfer-owner` succeeds.

- [ ] **Step 6: Commit** — `git commit -am "feat(api): org/app/member/provider-cred endpoints + per-org permissions"`

---

## Task 9: Scope existing data endpoints by tenant

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs`
- Test: `tests/multitenant_api.rs`

For each endpoint below, replace the old key_id-based or global call with the tenant-scoped variant, pulling `org_id`/`app_id` from `ctx` and returning 403 if `ctx.org_id` is None (hosted master/platform context on a tenant route). **Canonical change (apply to each):**

```rust
let org_id = ctx.org_id.as_deref().ok_or(ApiError::forbidden("no active organization"))?;
let app_id = ctx.app_id.as_deref();
let items = state.db.list_generations(org_id, app_id, page, per_page).await?;
```

- [ ] **Step 1:** `list_generations`/`count_generations` handler (`GET /v1/generations`) → tenant-scoped.
- [ ] **Step 2:** `GET /v1/logs` + `/v1/logs/{id}/artifact` → filter by org/app (artifact: verify the artifact's org matches).
- [ ] **Step 3:** `GET /v1/stats` → `get_stats(org_id, app_id)`.
- [ ] **Step 4:** `GET /v1/audit` → `list_audit_log` with `org_id` filter.
- [ ] **Step 5:** `GET /v1/keys` → `list_api_keys_for_app(ctx.app_id)`; `POST /v1/keys` already scoped in Task 5; rotate/revoke → verify the target key's `org_id == ctx.org_id` before acting (403 otherwise).
- [ ] **Step 6:** `generate_image`/`generate_video` → pass `ctx.org_id`/`ctx.app_id` into `insert_generation` + `log_request` so rows are stamped.
- [ ] **Step 7:** `GET /v1/videos/{id}` + `PATCH /v1/generations/{id}` (cancel) → load the generation, 404 if its `org_id != ctx.org_id`.
- [ ] **Step 8:** `cargo build --lib` clean (all Task-4 call sites now satisfied). Run `cargo test --lib` → PASS.
- [ ] **Step 9: Isolation test** — the comprehensive `cross_tenant_isolation` test asserts every one of these endpoints returns only the caller's org data and 403/404 across tenants.
- [ ] **Step 10: Commit** — `git commit -am "feat(api): scope generations/logs/stats/audit/keys/generate by org+app"`

---

## Task 10: Integration REST suite + `spawn_app()` harness

**Files:**
- Create: `litegen-core/tests/multitenant_api.rs`

- [ ] **Step 1: Write the harness** (real server, fresh tempfile SQLite, hosted mode, master key set so auth is enforced):

```rust
use std::sync::Arc;
struct TestApp { base: String, http: reqwest::Client, _tmp: tempfile::NamedTempFile }

async fn spawn_app() -> TestApp {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}?mode=rwc", tmp.path().display());
    let db: Arc<dyn litegen::db::DatabaseStore> =
        Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.unwrap());
    // Build AppState like build_test_state(), but hosted + a master key + a secrets key.
    let state = Arc::new(build_state(db, litegen::config::Mode::Hosted,
        Some("test-master".into()), Some([7u8;32])));
    let app = litegen::api::create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    TestApp {
        base: format!("http://{addr}"),
        http: reqwest::Client::builder().cookie_store(true).build().unwrap(),
        _tmp: tmp,
    }
}
```

(`build_state` is a local copy of `build_test_state()` parameterized by mode/master_key/secrets_key; reuse the constructors confirmed to exist: `ProviderRegistry::new()`, `AppConfig::default()`, `GenerationCache::new(&CacheGlobalConfig::default())`, `LocalStore`, `ProxyRouter::new`, `Materializer::new(Arc::new(NoopStorage), reqwest::Client::new())`, `CapabilityRegistry::from_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"))`, `RateLimiter::new()`, `InFlightLimit::new(64)`, `OAuthConfig::default()`. Use **per-test cookie jars** by building a fresh `reqwest::Client` per simulated user.)

- [ ] **Step 2: Write the matrix** — one `#[tokio::test]` each (full bodies):
  - `signup_creates_org_and_app` — POST `/v1/auth/signup` `{email,password,org_name}` → 200; cookie set; `GET /v1/orgs` len 1; `GET /v1/orgs/{id}/apps` len 1.
  - `two_signups_isolated_orgs` — two clients, two orgs; A's `GET /v1/orgs` never lists B.
  - `session_csrf_required` — mutating POST via cookie without `X-CSRF-Token` → 403; with token from `/v1/auth/csrf` → 200.
  - `create_key_returns_id_and_secret_once` — `pk_live_`/`sk_live_`; list shows public_id + prefix, never secret.
  - `bearer_secret_generates` — `Authorization: Bearer sk_live_…` → `POST /v1/images/generations` (mock model) → 200; the gen row carries the key's org/app.
  - `cross_tenant_isolation` — the full 403/404 matrix across keys/generations/logs/audit + the `X-Litegen-Org-Id` spoof → 403.
  - `invite_and_role_enforcement` — owner invites Member (token via dev flag) → accept → Member mutating org/members → 403; Owner transfer-owner → roles swap.
  - `revoked_key_401` and `quota_exhausted_402`.
  - `hosted_master_key_is_platform_admin` — no auth → 401; master key on `GET /v1/keys` (tenant route, null org) → 403; on `/v1/admin/*` (add a trivial `/v1/admin/orgs` listing) → 200.

- [ ] **Step 3: Run** `cargo test --test multitenant_api` → all PASS.

- [ ] **Step 4: Commit** — `git commit -am "test(integration): real-HTTP multi-tenant auth + isolation suite"`

---

## Task 11: OpenAPI annotations + SDK regen

**Files:**
- Modify: `litegen-core/src/api/openapi.rs` (register new schemas + paths)
- Regenerate: `sdks/typescript`, `sdks/python`

- [ ] **Step 1:** Ensure every new handler has `#[utoipa::path(...)]` and every new type derives `ToSchema` (done in Tasks 3/8). Add them to the `#[derive(OpenApi)] #[openapi(paths(...), components(schemas(...)))]` list in `openapi.rs`.
- [ ] **Step 2:** `cd litegen-core && cargo build --release` (so the binary serves the new `/openapi.json`).
- [ ] **Step 3:** `bash sdks/scripts/regen-all.sh` (regenerates TS + Python from the live/openapi spec — follow its README; it boots the binary or reads a dumped spec). Resolve any codegen errors.
- [ ] **Step 4:** `cd sdks/typescript && npm run build && npm test` → PASS; verify new resources exist (`client.orgs`, `client.apps`, `client.members`, `client.keys` with `public_id`).
- [ ] **Step 5: Commit** — `git commit -am "feat(sdk): regenerate TS/Python SDKs for org/app/member/key endpoints"`

---

## Task 12: Dashboard — tenant context, switcher, onboarding, scoped pages

**Files:**
- Create: `dashboard/src/context/TenantContext.tsx`, `components/OrgSwitcher.tsx`, `pages/Organization.tsx`, `pages/Members.tsx`, `pages/Apps.tsx`
- Modify: `dashboard/src/sdk-client.ts`, `App.tsx`, `pages/Signup.tsx`, `pages/Keys.tsx`, `components/UserMenu.tsx`

- [ ] **Step 1: SDK header injection.** In `sdk-client.ts`, read the active org/app from `localStorage` (`litegen_active_org`, `litegen_active_app`) and pass them as default headers `X-Litegen-Org-Id`/`X-Litegen-App-Id` on every request (the SDK supports a header hook / `fetchOverride`). Add `setActiveTenant(orgId, appId)` that updates localStorage + clears caches.
- [ ] **Step 2: TenantContext** — provider above `<Routes>` in `App.tsx`: on mount calls `client.auth.me()` → stores `orgs`, `activeOrg`, `activeApp`; exposes `switchOrg/switchApp`. Persists per-origin.
- [ ] **Step 3: OrgSwitcher** in `UserMenu` — org dropdown → app dropdown; selecting calls `setActiveTenant` + refetches.
- [ ] **Step 4: Onboarding** — `Signup.tsx` adds an `org-name` field (`data-testid="signup-org-name"`); post-signup, if the org has no extra app, show a "create your first app" step (reuse `Apps.tsx`).
- [ ] **Step 5: Scoped pages** — Overview/Logs/Generations/Keys read the active app from context (already injected via headers, so calls are unchanged but must refetch on switch). `Keys.tsx` shows `public_id` next to the prefix and keeps the secret-once banner. New `Members.tsx` (list/invite/role/transfer, org-perm gated) and `Organization.tsx` (rename, apps list, provider-credentials form).
- [ ] **Step 6: Build** — `cd dashboard && npm run build` → clean.
- [ ] **Step 7: Commit** — `git commit -am "feat(dashboard): org/app context + switcher + onboarding + members/org pages"`

---

## Task 13: Playwright e2e — recreate DB each run + multi-tenant flow

**Files:**
- Modify: `dashboard/playwright.config.ts`, `dashboard/vite.config.ts`
- Create: `dashboard/e2e/multitenant.spec.ts`

- [ ] **Step 1: Recreate DB each run + hosted mode.** In `playwright.config.ts` `webServer[0]`, change the backend env to a **fresh temp file DB deleted on each start** and hosted mode, and add a `cwd` so embedded migrations resolve:

```ts
{
  command: 'rm -f /tmp/litegen-e2e.db && exec ' + BINARY_PATH,
  cwd: path.resolve(__dirname, '../litegen-core'),
  url: 'http://127.0.0.1:5099/health',
  reuseExistingServer: false,
  timeout: 60_000,
  env: {
    LITEGEN__SERVER__HOST: '127.0.0.1',
    LITEGEN__SERVER__PORT: '5099',
    LITEGEN__DATABASE_URL: 'sqlite:///tmp/litegen-e2e.db?mode=rwc',
    LITEGEN__MODE: 'hosted',
    LITEGEN__MASTER_KEY: MASTER_KEY,
    LITEGEN__SECRETS_KEY: 'MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=', // 32 bytes base64
    LITEGEN_MODELS_DIR: MODELS_DIR,
    LITEGEN_CORS_ORIGINS: 'http://localhost:5174,http://127.0.0.1:5174',
    LITEGEN__CORS__ALLOW_CREDENTIALS: 'true',
    LITEGEN__COOKIE_INSECURE_DEV: 'true',
    LITEGEN__DEV__EXPOSE_INVITE_TOKENS: 'true',
  },
}
```

(Using a temp **file** with `?mode=rwc` + `rm -f` on start guarantees a clean DB each run and avoids the `:memory:` pooled-connection pitfall. The binary auto-migrates the fresh file on boot.)

- [ ] **Step 2: Keep the dev-server proxy** (`webServer[1]` `npm run dev … --port 5174` with `VITE_PROXY_TARGET=http://127.0.0.1:5099`, `VITE_API_URL=http://127.0.0.1:5174`) so session cookies stay same-origin. (No change needed; documented here so the e2e author doesn't switch to `preview` and lose the proxy.)
- [ ] **Step 3: Write `multitenant.spec.ts`** driving the UI on `@litegen/sdk`, with unique emails/slugs per run:
  1. Sign up org A owner (fill `signup-org-name`) → land in app; assert UserMenu email + an OrgSwitcher showing org A.
  2. Create a second app via Apps page; create an API key → assert secret shown once (`sk_live_`) and `public_id` (`pk_live_`) visible.
  3. (Provider cred form) add a `mock` credential; assert it lists without revealing the secret.
  4. Invite a Member (read `invite-dev-token`), accept in a fresh `context`, sign in as Member → assert Members management is hidden/forbidden (role-gated).
  5. Use the OrgSwitcher.
  6. In another `context`, sign up org B; assert org B sees none of org A's apps/keys (navigate + count 0).
  7. Use the captured key via `page.request.post('http://127.0.0.1:5099/v1/images/generations', { headers:{Authorization:`Bearer ${secret}`}, data:{model:'mock/...',prompt:'x'} })` → 200, then confirm the generation appears in the dashboard Logs scoped to that app.
- [ ] **Step 4: Run** `cd litegen-core && cargo build --release && cd ../dashboard && npx playwright test multitenant` → PASS with a freshly recreated DB. Keep `god-test.spec.ts` green under default single-tenant by giving it its own backend config or running it against `LITEGEN__MODE=single_tenant` (the existing god test still uses master-key + closed signup — verify it passes or adjust its first signup block to the hosted flow).
- [ ] **Step 5: Commit** — `git commit -am "test(e2e): multi-tenant Playwright flow + DB-recreate-each-run harness"`

---

## Task 14: Polish + docs

- [ ] **Step 1:** `cd litegen-core && cargo clippy --all-targets -- -D warnings` → clean (fix warnings).
- [ ] **Step 2:** `cargo build --release`, `cargo test`, `cd ../dashboard && npm run build`, `cd ../sdks/typescript && npm run build && npm test` → all clean.
- [ ] **Step 3:** README: add a "Hosting / multi-tenant" section documenting `LITEGEN__MODE`, `LITEGEN__SECRETS_KEY`, the org→app→key model, `Authorization: Bearer sk_live_…`, and `X-Litegen-Org-Id`/`X-Litegen-App-Id`.
- [ ] **Step 4: Commit** — `git commit -am "docs+chore: multi-tenant README + clippy clean"`

---

## Self-Review (completed)

- **Spec coverage:** schema (T2), mode switch (T1/T6), id/secret keys (T3/T5), tenant middleware + isolation (T6/T9/T10), signup→org (T7), org/app/member/invite endpoints + per-org perms (T8), provider-cred schema + encrypted storage (T2/T5/T8; per-request *use* is Phase 2 per spec), SDK regen (T11), dashboard (T12), integration tests (T10) + e2e with DB recreate (T13), backward compat via `single_tenant` (T1/T6, god test in T13). ✓
- **Placeholder scan:** the only deferred item is per-request BYO credential *threading*, which the spec explicitly scopes to Phase 2; its schema + storage are fully specified here. No TBDs in steps. ✓
- **Type consistency:** `KeyContext.{org_id,app_id}: Option<String>`, `Organization/Application` ids are `String`, `create_api_key(org_id, app_id, public_id, name, …)` signature is used identically in T4 (def), T5 (handler), and T10 (test). `list_generations(org_id: &str, app_id: Option<&str>, …)` consistent across T4/T9/T10. `Role::parse`/`as_str` (not FromStr) used throughout. ✓

---

## Execution note (Phase boundaries)

This plan is **Phase 1 only**. Phase 2 (per-request BYO provider-credential threading), Phase 3 (Redis/poller-lease/object-storage for multi-instance), and Phase 4 (DO infra provisioning, **$-gated**) each get their own plan after Phase 1 is green.
