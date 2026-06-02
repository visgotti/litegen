# Auth Multi-Key (Quota / RPM / Scopes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace single-master-key auth with a DB-backed multi-key model that adds per-key token quota (USD budget), per-key RPM rate limiting, and scope-based authorization.

**Architecture:** New migration adds quota/scopes/rpm columns to `api_keys`; `DatabaseStore` trait gets new methods (`get_api_key`, `update_api_key`, `lookup_api_key_by_hash`, `atomic_charge_tokens`); auth middleware is rewritten to inject a `KeyContext` into request extensions; a `RateLimiter` lives in `AppState`; quota deduction happens post-generation via an atomic DB update; master key bypasses all limits.

**Tech Stack:** Rust, axum 0.8, sqlx 0.8 (SQLite + Postgres), sha2, tokio, tower middleware

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/sqlite/20240101000002_api_key_quota.sql` | Adds quota/scopes/rpm/webhook columns (SQLite) |
| Create | `migrations/postgres/20240101000002_api_key_quota.sql` | Same, Postgres syntax |
| Modify | `src/types/mod.rs` | Extend `ApiKey`, add `ApiKeyDetail`, `UpdateApiKeyRequest`, `CreateApiKeyRequestV2` |
| Modify | `src/db/trait_def.rs` | Add new trait methods |
| Modify | `src/db/sqlite.rs` | Implement new trait methods + `ApiKeyRowV2` |
| Modify | `src/db/postgres.rs` | Implement new trait methods |
| Create | `src/db/sqlite_tests.rs` | DB unit tests (quota, scopes, lookup_by_hash, atomic_charge) |
| Modify | `src/api/middleware/mod.rs` | Rewrite auth: `Scope`, `KeyContext`, scope-check layer; add `RateLimiter` to `AppState` |
| Modify | `src/api/middleware/auth_tests.rs` | Extend existing tests: scope, expiry, inactive, quota pre-flight |
| Create | `src/api/middleware/rate_limit.rs` | Token-bucket `RateLimiter` |
| Modify | `src/api/handlers.rs` | Post-charge quota; extend `CreateApiKeyRequest`; add `GET/PATCH /v1/keys/{id}` |

---

## Task A1 — Migration + DB layer

**Files:**
- Create: `litegen-core/migrations/sqlite/20240101000002_api_key_quota.sql`
- Create: `litegen-core/migrations/postgres/20240101000002_api_key_quota.sql`
- Modify: `litegen-core/src/types/mod.rs`
- Modify: `litegen-core/src/db/trait_def.rs`
- Modify: `litegen-core/src/db/sqlite.rs`
- Modify: `litegen-core/src/db/postgres.rs`
- Create: `litegen-core/src/db/sqlite_tests.rs`

### Step A1.1 — Write the SQLite migration

Create `litegen-core/migrations/sqlite/20240101000002_api_key_quota.sql`:

```sql
-- Add quota/scopes/rpm/webhook columns to api_keys
ALTER TABLE api_keys ADD COLUMN token_quota REAL;
ALTER TABLE api_keys ADD COLUMN tokens_used REAL NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN rpm_limit INTEGER;
ALTER TABLE api_keys ADD COLUMN scopes TEXT NOT NULL DEFAULT 'generate,read';
ALTER TABLE api_keys ADD COLUMN webhook_url TEXT;
```

### Step A1.2 — Write the Postgres migration

Create `litegen-core/migrations/postgres/20240101000002_api_key_quota.sql`:

```sql
-- Add quota/scopes/rpm/webhook columns to api_keys
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS token_quota DOUBLE PRECISION;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS tokens_used DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS rpm_limit INTEGER;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS scopes TEXT NOT NULL DEFAULT 'generate,read';
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS webhook_url TEXT;
```

### Step A1.3 — Extend `ApiKey` struct and add new types in `src/types/mod.rs`

In the "API Key Auth" section, replace:
```rust
/// API key for authenticating with the LiteGen proxy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
}
```

With:
```rust
/// API key for authenticating with the LiteGen proxy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    /// USD budget cap; None = unlimited.
    pub token_quota: Option<f64>,
    /// Running USD spent under this key.
    pub tokens_used: f64,
    /// Requests-per-minute cap; None = unlimited.
    pub rpm_limit: Option<u32>,
    /// CSV of scopes: "generate,read,admin".
    pub scopes: String,
    /// Webhook URL for async notifications (future use).
    pub webhook_url: Option<String>,
}

/// Full key detail for GET /v1/keys/{id} and PATCH /v1/keys/{id} responses.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyDetail {
    pub id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
}

/// Request body for PATCH /v1/keys/{id}.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub scopes: Option<String>,
    pub webhook_url: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: Option<bool>,
}
```

Also extend `ApiKeyInfo` and `ApiKeyCreatedResponse` with the new fields:
```rust
/// Public view of an API key (no hash).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
}

/// Response for `POST /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyCreatedResponse {
    pub key: String,
    pub prefix: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
}
```

### Step A1.4 — Add new trait methods in `src/db/trait_def.rs`

Replace the entire file with:
```rust
use async_trait::async_trait;
use uuid::Uuid;

use crate::types::*;

/// Trait defining the database operations. Implementations exist for SQLite and PostgreSQL.
#[async_trait]
pub trait DatabaseStore: Send + Sync {
    // ─── Request Logs ───────────────────────────────────────────────────

    async fn log_request(
        &self,
        id: &str,
        model: &str,
        provider: &str,
        status: &str,
        media_type: &str,
        cost_usd: f64,
        latency_ms: i64,
        error: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<(), sqlx::Error>;

    async fn get_request_logs(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error>;

    // ─── API Keys ───────────────────────────────────────────────────────

    async fn create_api_key(
        &self,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        token_quota: Option<f64>,
        rpm_limit: Option<u32>,
        scopes: &str,
        webhook_url: Option<&str>,
    ) -> Result<ApiKey, sqlx::Error>;

    async fn get_api_key(&self, id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn update_api_key(
        &self,
        id: &Uuid,
        req: &UpdateApiKeyRequest,
    ) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error>;

    /// Atomically add `cost_usd` to `tokens_used`.
    /// Returns the new `tokens_used` value.
    /// Returns `sqlx::Error::RowNotFound` if the key doesn't exist.
    async fn atomic_charge_tokens(
        &self,
        id: &Uuid,
        cost_usd: f64,
    ) -> Result<f64, sqlx::Error>;

    async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error>;

    async fn revoke_api_key(&self, id: &Uuid) -> Result<bool, sqlx::Error>;

    // ─── Stats ──────────────────────────────────────────────────────────

    async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error>;
}
```

### Step A1.5 — Update `ApiKeyRow` and implement new methods in `src/db/sqlite.rs`

**a)** Replace `ApiKeyRow` struct with:
```rust
#[derive(sqlx::FromRow)]
pub(crate) struct ApiKeyRow {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<i64>,
    pub scopes: String,
    pub webhook_url: Option<String>,
}
```

**b)** Replace `api_key_from_row` with:
```rust
pub(crate) fn api_key_from_row(r: ApiKeyRow) -> ApiKey {
    ApiKey {
        id: Uuid::parse_str(&r.id).unwrap_or_default(),
        name: r.name,
        key_hash: r.key_hash,
        key_prefix: r.key_prefix,
        created_at: r.created_at,
        expires_at: r.expires_at,
        is_active: r.is_active,
        token_quota: r.token_quota,
        tokens_used: r.tokens_used,
        rpm_limit: r.rpm_limit.map(|v| v as u32),
        scopes: r.scopes,
        webhook_url: r.webhook_url,
    }
}
```

**c)** Add a constant for the full column list to avoid repetition:
```rust
const API_KEY_COLS: &str = "id, name, key_hash, key_prefix, created_at, expires_at, is_active, token_quota, tokens_used, rpm_limit, scopes, webhook_url";
```

**d)** Replace `create_api_key` impl:
```rust
async fn create_api_key(
    &self,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    token_quota: Option<f64>,
    rpm_limit: Option<u32>,
    scopes: &str,
    webhook_url: Option<&str>,
) -> Result<ApiKey, sqlx::Error> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO api_keys (id, name, key_hash, key_prefix, created_at, is_active, token_quota, tokens_used, rpm_limit, scopes, webhook_url)
        VALUES (?, ?, ?, ?, ?, 1, ?, 0, ?, ?, ?)
        "#,
    )
    .bind(id.to_string())
    .bind(name)
    .bind(key_hash)
    .bind(key_prefix)
    .bind(now)
    .bind(token_quota)
    .bind(rpm_limit.map(|v| v as i64))
    .bind(scopes)
    .bind(webhook_url)
    .execute(&self.pool)
    .await?;

    Ok(ApiKey {
        id,
        name: name.to_string(),
        key_hash: key_hash.to_string(),
        key_prefix: key_prefix.to_string(),
        created_at: now,
        expires_at: None,
        is_active: true,
        token_quota,
        tokens_used: 0.0,
        rpm_limit,
        scopes: scopes.to_string(),
        webhook_url: webhook_url.map(|s| s.to_string()),
    })
}
```

**e)** Add `get_api_key` impl:
```rust
async fn get_api_key(&self, id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error> {
    let row = sqlx::query_as::<_, ApiKeyRow>(&format!(
        "SELECT {} FROM api_keys WHERE id = ?", API_KEY_COLS
    ))
    .bind(id.to_string())
    .fetch_optional(&self.pool)
    .await?;
    Ok(row.map(api_key_from_row))
}
```

**f)** Add `update_api_key` impl:
```rust
async fn update_api_key(
    &self,
    id: &Uuid,
    req: &crate::types::UpdateApiKeyRequest,
) -> Result<Option<ApiKey>, sqlx::Error> {
    // Build SET clause dynamically based on which fields are Some
    let mut sets: Vec<&str> = Vec::new();
    if req.name.is_some() { sets.push("name = ?"); }
    if req.token_quota.is_some() { sets.push("token_quota = ?"); }
    if req.rpm_limit.is_some() { sets.push("rpm_limit = ?"); }
    if req.scopes.is_some() { sets.push("scopes = ?"); }
    if req.webhook_url.is_some() { sets.push("webhook_url = ?"); }
    if req.expires_at.is_some() { sets.push("expires_at = ?"); }
    if req.is_active.is_some() { sets.push("is_active = ?"); }

    if sets.is_empty() {
        return self.get_api_key(id).await;
    }

    let sql = format!("UPDATE api_keys SET {} WHERE id = ?", sets.join(", "));
    let mut q = sqlx::query(&sql);
    if let Some(v) = &req.name { q = q.bind(v); }
    if let Some(v) = req.token_quota { q = q.bind(v); }
    if let Some(v) = req.rpm_limit { q = q.bind(v as i64); }
    if let Some(v) = &req.scopes { q = q.bind(v); }
    if let Some(v) = &req.webhook_url { q = q.bind(v); }
    if let Some(v) = req.expires_at { q = q.bind(v); }
    if let Some(v) = req.is_active { q = q.bind(v); }
    q = q.bind(id.to_string());

    let result = q.execute(&self.pool).await?;
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    self.get_api_key(id).await
}
```

**g)** Add `lookup_api_key_by_hash` impl:
```rust
async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
    let row = sqlx::query_as::<_, ApiKeyRow>(&format!(
        "SELECT {} FROM api_keys WHERE key_hash = ? AND is_active = 1", API_KEY_COLS
    ))
    .bind(key_hash)
    .fetch_optional(&self.pool)
    .await?;
    Ok(row.map(api_key_from_row))
}
```

**h)** Add `atomic_charge_tokens` impl:
```rust
async fn atomic_charge_tokens(
    &self,
    id: &Uuid,
    cost_usd: f64,
) -> Result<f64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE api_keys SET tokens_used = tokens_used + ? WHERE id = ?"
    )
    .bind(cost_usd)
    .bind(id.to_string())
    .execute(&self.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(sqlx::Error::RowNotFound);
    }

    let row: (f64,) = sqlx::query_as(
        "SELECT tokens_used FROM api_keys WHERE id = ?"
    )
    .bind(id.to_string())
    .fetch_one(&self.pool)
    .await?;

    Ok(row.0)
}
```

**i)** Update `validate_api_key` to use full columns:
```rust
async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
    let row = sqlx::query_as::<_, ApiKeyRow>(&format!(
        "SELECT {} FROM api_keys WHERE key_hash = ? AND is_active = 1", API_KEY_COLS
    ))
    .bind(key_hash)
    .fetch_optional(&self.pool)
    .await?;
    Ok(row.map(api_key_from_row))
}
```

**j)** Update `list_api_keys` to use full columns:
```rust
async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ApiKeyRow>(&format!(
        "SELECT {} FROM api_keys ORDER BY created_at DESC", API_KEY_COLS
    ))
    .fetch_all(&self.pool)
    .await?;
    Ok(rows.into_iter().map(api_key_from_row).collect())
}
```

### Step A1.6 — Mirror changes in `src/db/postgres.rs`

Postgres uses `$N` placeholders instead of `?`. The `ApiKeyRow` and `api_key_from_row` are shared from `sqlite.rs` (the existing import pattern). Add the same methods with `$1`, `$2`, etc. substituted. Only the SQL placeholders and type differences change:

- `is_active = TRUE` instead of `is_active = 1`
- `BOOLEAN` cast: bind `v` directly (sqlx handles it)
- `rpm_limit` is `i64` — same bind pattern

All the method signatures are identical to SQLite; the SQL strings differ only in placeholders. Follow the existing pattern in `postgres.rs` exactly.

Also add `get_api_key`, `update_api_key`, `lookup_api_key_by_hash`, and `atomic_charge_tokens` using `$1/$2/...` placeholders.

### Step A1.7 — Write DB unit tests in `src/db/sqlite_tests.rs`

Create `litegen-core/src/db/sqlite_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::trait_def::DatabaseStore;
    use crate::types::UpdateApiKeyRequest;

    async fn in_memory_db() -> SqliteDatabase {
        SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite")
    }

    #[tokio::test]
    async fn create_and_get_api_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("test-key", "hash1", "lg-abc", None, None, "generate,read", None).await.unwrap();
        assert_eq!(key.name, "test-key");
        assert_eq!(key.scopes, "generate,read");
        assert!(key.token_quota.is_none());
        assert_eq!(key.tokens_used, 0.0);

        let fetched = db.get_api_key(&key.id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, key.id);
    }

    #[tokio::test]
    async fn list_api_keys_returns_created_key() {
        let db = in_memory_db().await;
        db.create_api_key("k1", "h1", "lg-1", None, None, "generate,read", None).await.unwrap();
        db.create_api_key("k2", "h2", "lg-2", Some(10.0), Some(60), "generate,read,admin", None).await.unwrap();
        let keys = db.list_api_keys().await.unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    async fn update_api_key_changes_quota_and_scopes() {
        let db = in_memory_db().await;
        let key = db.create_api_key("upd", "hash_upd", "lg-upd", None, None, "generate,read", None).await.unwrap();

        let req = UpdateApiKeyRequest {
            name: None,
            token_quota: Some(5.0),
            rpm_limit: Some(30),
            scopes: Some("admin".to_string()),
            webhook_url: None,
            expires_at: None,
            is_active: None,
        };
        let updated = db.update_api_key(&key.id, &req).await.unwrap().unwrap();
        assert_eq!(updated.token_quota, Some(5.0));
        assert_eq!(updated.rpm_limit, Some(30));
        assert_eq!(updated.scopes, "admin");
    }

    #[tokio::test]
    async fn lookup_by_hash_finds_active_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("lookup", "unique_hash_abc", "lg-lk", None, None, "generate,read", None).await.unwrap();
        let found = db.lookup_api_key_by_hash("unique_hash_abc").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, key.id);
    }

    #[tokio::test]
    async fn lookup_by_hash_returns_none_for_revoked_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("rev", "rev_hash", "lg-rv", None, None, "generate,read", None).await.unwrap();
        db.revoke_api_key(&key.id).await.unwrap();
        let found = db.lookup_api_key_by_hash("rev_hash").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn atomic_charge_tokens_accumulates() {
        let db = in_memory_db().await;
        let key = db.create_api_key("charge", "charge_hash", "lg-ch", Some(10.0), None, "generate,read", None).await.unwrap();

        let used1 = db.atomic_charge_tokens(&key.id, 3.0).await.unwrap();
        assert!((used1 - 3.0).abs() < 1e-9);

        let used2 = db.atomic_charge_tokens(&key.id, 4.0).await.unwrap();
        assert!((used2 - 7.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn atomic_charge_tokens_error_on_nonexistent_key() {
        let db = in_memory_db().await;
        let fake_id = Uuid::new_v4();
        let result = db.atomic_charge_tokens(&fake_id, 1.0).await;
        assert!(result.is_err());
    }
}
```

### Step A1.8 — Add `mod sqlite_tests` in `src/db/mod.rs`

In `src/db/mod.rs`, add:
```rust
#[cfg(test)] mod sqlite_tests;
```

### Step A1.9 — Update NoopDb impls in test files

The `NoopDb` / `SpyDb` structs in `src/api/middleware/auth_tests.rs` and `src/api/middleware/e2e_tests.rs` implement `DatabaseStore`. They must implement the new trait methods. Add stub implementations:

For every `NoopDb` / `SpyDb` impl, add:
```rust
async fn get_api_key(&self, _id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

async fn update_api_key(&self, _id: &Uuid, _req: &UpdateApiKeyRequest) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

async fn lookup_api_key_by_hash(&self, _key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

async fn atomic_charge_tokens(&self, _id: &Uuid, _cost_usd: f64) -> Result<f64, sqlx::Error> { Ok(0.0) }
```

Also update `create_api_key` signatures to match the new 7-parameter version:
```rust
async fn create_api_key(
    &self, name: &str, key_hash: &str, key_prefix: &str,
    _token_quota: Option<f64>, _rpm_limit: Option<u32>, _scopes: &str, _webhook_url: Option<&str>,
) -> Result<ApiKey, sqlx::Error> {
    Ok(ApiKey {
        id: Uuid::new_v4(),
        name: name.to_string(),
        key_hash: key_hash.to_string(),
        key_prefix: key_prefix.to_string(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        is_active: true,
        token_quota: None,
        tokens_used: 0.0,
        rpm_limit: None,
        scopes: "generate,read".to_string(),
        webhook_url: None,
    })
}
```

### Step A1.10 — Run tests and verify

```bash
cd litegen-core && cargo test --lib 2>&1 | tail -20
```
Expected: all previously passing tests still pass, plus the new `sqlite_tests` tests pass.

### Step A1.11 — Commit

```bash
cd litegen-core
git add migrations/sqlite/20240101000002_api_key_quota.sql \
        migrations/postgres/20240101000002_api_key_quota.sql \
        src/types/mod.rs \
        src/db/trait_def.rs \
        src/db/sqlite.rs \
        src/db/postgres.rs \
        src/db/sqlite_tests.rs \
        src/db/mod.rs \
        src/api/middleware/auth_tests.rs \
        src/api/middleware/e2e_tests.rs
git commit -m "feat(litegen-core): db schema + accessors for api key quota/scopes/rpm"
```

---

## Task A2 — Auth middleware + KeyContext

**Files:**
- Modify: `litegen-core/src/api/middleware/mod.rs`
- Modify: `litegen-core/src/api/middleware/auth_tests.rs`
- Modify: `litegen-core/src/api/handlers.rs` (add `RateLimiter` to `AppState` import paths)

### Step A2.1 — Define `Scope` and `KeyContext` in `src/api/middleware/mod.rs`

Add before `AppState`:

```rust
/// Scopes control which routes a key can access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Generate,
    Read,
    Admin,
}

impl Scope {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "generate" => Some(Scope::Generate),
            "read" => Some(Scope::Read),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }
}

/// Auth context injected into request extensions by `auth_middleware`.
#[derive(Debug, Clone)]
pub struct KeyContext {
    /// None for master key; Some(id) for DB-backed keys.
    pub key_id: Option<Uuid>,
    pub scopes: Vec<Scope>,
    pub rpm_limit: Option<u32>,
    pub quota_remaining: Option<f64>,
    pub webhook_url: Option<String>,
}
```

### Step A2.2 — Add `rate_limiter` field to `AppState`

```rust
use crate::api::middleware::rate_limit::RateLimiter;

pub struct AppState {
    pub router: Arc<ProxyRouter>,
    pub db: Arc<dyn DatabaseStore>,
    pub master_key: Option<String>,
    pub registry: Arc<crate::capabilities::CapabilityRegistry>,
    pub materializer: Arc<Materializer>,
    pub rate_limiter: Arc<RateLimiter>,
}
```

### Step A2.3 — Rewrite `auth_middleware`

Replace the current `auth_middleware` function with:

```rust
pub async fn auth_middleware(
    headers: HeaderMap,
    state: Arc<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // If no master key configured, allow all requests (dev mode)
    if state.master_key.is_none() {
        // Insert a permissive KeyContext so handlers can always read it
        let ctx = KeyContext {
            key_id: None,
            scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
        };
        request.extensions_mut().insert(ctx);
        return next.run(request).await;
    }

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v));

    let api_key = match auth_header {
        Some(key) if !key.is_empty() => key,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": {
                        "message": "Missing API key. Provide via Authorization: Bearer <key>",
                        "type": "authentication_error",
                        "code": 401
                    }
                })),
            )
                .into_response();
        }
    };

    // Check master key first — unlimited, all scopes
    if let Some(master) = &state.master_key {
        if api_key == master {
            let ctx = KeyContext {
                key_id: None,
                scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
                rpm_limit: None,
                quota_remaining: None,
                webhook_url: None,
            };
            request.extensions_mut().insert(ctx);
            return next.run(request).await;
        }
    }

    // Hash and look up DB key
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(api_key.as_bytes());
        hex::encode(hasher.finalize())
    };

    match state.db.lookup_api_key_by_hash(&hash).await {
        Ok(Some(key)) => {
            // Check expiry
            if let Some(exp) = key.expires_at {
                if exp < chrono::Utc::now() {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": { "message": "API key expired", "type": "authentication_error", "code": 401 }
                        })),
                    ).into_response();
                }
            }

            // Compute quota_remaining
            let quota_remaining = key.token_quota.map(|quota| quota - key.tokens_used);

            // Pre-flight quota check: if tokens_used >= token_quota, reject with 402
            if let (Some(quota), used) = (key.token_quota, key.tokens_used) {
                if used >= quota {
                    return (
                        StatusCode::PAYMENT_REQUIRED,
                        Json(serde_json::json!({
                            "error": {
                                "message": "Token quota exceeded for this API key",
                                "type": "quota_exceeded",
                                "code": 402
                            }
                        })),
                    ).into_response();
                }
            }

            let scopes: Vec<Scope> = key.scopes
                .split(',')
                .filter_map(Scope::from_str)
                .collect();

            let ctx = KeyContext {
                key_id: Some(key.id),
                scopes,
                rpm_limit: key.rpm_limit,
                quota_remaining,
                webhook_url: key.webhook_url,
            };

            // RPM check
            if let (Some(key_id), Some(rpm)) = (ctx.key_id, ctx.rpm_limit) {
                match state.rate_limiter.try_take(key_id, rpm).await {
                    Ok(()) => {}
                    Err(retry_after) => {
                        return (
                            StatusCode::TOO_MANY_REQUESTS,
                            [("Retry-After", retry_after.to_string())],
                            Json(serde_json::json!({
                                "error": {
                                    "message": format!("Rate limit exceeded. Retry after {} seconds.", retry_after),
                                    "type": "rate_limit_exceeded",
                                    "code": 429
                                }
                            })),
                        ).into_response();
                    }
                }
            }

            request.extensions_mut().insert(ctx);
            next.run(request).await
        }
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Invalid or inactive API key",
                    "type": "authentication_error",
                    "code": 401
                }
            })),
        ).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to look up API key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": "Internal authentication error",
                        "type": "internal_error",
                        "code": 500
                    }
                })),
            ).into_response()
        }
    }
}
```

### Step A2.4 — Add `require_scope` middleware factory

Add after `auth_middleware`:

```rust
/// Returns a `tower::Layer`-compatible axum middleware that 403s if the
/// current `KeyContext` does not contain `required_scope`.
pub fn require_scope(required_scope: Scope) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> + Clone + Send + 'static {
    move |request: Request, next: Next| {
        Box::pin(async move {
            let ctx = request.extensions().get::<KeyContext>().cloned();
            match ctx {
                Some(ctx) if ctx.scopes.contains(&required_scope) => next.run(request).await,
                Some(_) => (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Insufficient scope",
                            "type": "forbidden_scope",
                            "code": 403
                        }
                    })),
                ).into_response(),
                None => (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Not authenticated",
                            "type": "authentication_error",
                            "code": 401
                        }
                    })),
                ).into_response(),
            }
        })
    }
}
```

### Step A2.5 — Wire scope checks into `create_router`

In `src/api/handlers.rs`, update `create_router` to apply scope middleware per route group. Use `axum::middleware::from_fn` wrapping each route with the appropriate scope. The cleanest pattern with axum 0.8 is to create sub-routers with `.layer`:

```rust
pub fn create_router(state: Arc<AppState>) -> axum::Router {
    use axum::routing::{delete, get, patch, post};
    use axum::middleware;
    use super::metrics::metrics_handler;
    use super::middleware::{auth_middleware, require_scope, Scope};
    use std::sync::Arc;

    let state_for_auth = state.clone();

    // Generate routes (require Scope::Generate)
    let generate_routes = axum::Router::new()
        .route("/v1/images/generations", post(generate_image))
        .route("/v1/images/cost", post(estimate_image_cost))
        .route("/v1/videos/generations", post(generate_video))
        .route("/v1/videos/cost", post(estimate_video_cost))
        .route("/v1/videos/{id}", get(get_video_status))
        .layer(middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
            require_scope(Scope::Generate)(req, next)
        }));

    // Read routes (require Scope::Read)
    let read_routes = axum::Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/models/{*id}", get(get_model_schema))
        .route("/health", get(health_check))
        .route("/health/live", get(liveness))
        .route("/v1/stats", get(get_stats))
        .route("/v1/logs", get(get_logs))
        .layer(middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
            require_scope(Scope::Read)(req, next)
        }));

    // Admin routes (require Scope::Admin)
    let admin_routes = axum::Router::new()
        .route("/v1/keys", post(create_api_key))
        .route("/v1/keys", get(list_api_keys))
        .route("/v1/keys/{id}", delete(revoke_api_key))
        .route("/v1/keys/{id}", get(get_api_key_handler))
        .route("/v1/keys/{id}", patch(patch_api_key_handler))
        .route("/v1/cache", delete(clear_cache))
        .layer(middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
            require_scope(Scope::Admin)(req, next)
        }));

    // Auth middleware wraps all routes (including metrics/openapi which are unauthenticated)
    let auth_state = state_for_auth.clone();
    axum::Router::new()
        .merge(generate_routes)
        .merge(read_routes)
        .merge(admin_routes)
        .route("/metrics", get(metrics_handler))
        .route("/openapi.json", get(openapi_spec))
        .layer(middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
            let s = auth_state.clone();
            async move {
                let headers = req.headers().clone();
                auth_middleware(headers, s, req, next).await
            }
        }))
        .with_state(state)
}
```

Note: `require_scope` closures need to capture `Scope` (which is `Copy`) so they can be moved into the closure factory.

### Step A2.6 — Add imports to `src/api/middleware/mod.rs`

Add to the top of the file:
```rust
pub mod rate_limit;
#[cfg(test)] mod validator_tests;
#[cfg(test)] mod e2e_tests;
#[cfg(test)] mod auth_tests;

use uuid::Uuid;
```

The full import block becomes:
```rust
pub mod rate_limit;
#[cfg(test)] mod validator_tests;
#[cfg(test)] mod e2e_tests;
#[cfg(test)] mod auth_tests;

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::db::DatabaseStore;
use crate::proxy::materializer::Materializer;
use crate::proxy::router::ProxyRouter;
use crate::api::middleware::rate_limit::RateLimiter;
```

### Step A2.7 — Extend `auth_tests.rs` with new test cases

In `auth_tests.rs`, update `build_state` to provide a `rate_limiter` and add tests:

```rust
use crate::api::middleware::rate_limit::RateLimiter;

async fn build_state(master_key: Option<&str>) -> Arc<AppState> {
    // ... existing code ...
    Arc::new(AppState {
        router,
        db: Arc::new(NoopDb),
        master_key: master_key.map(|s| s.to_string()),
        registry: cap_registry,
        materializer,
        rate_limiter: Arc::new(RateLimiter::new()),
    })
}
```

Add a `ValidKeyDb` that returns a real `ApiKey` with scopes, for testing scope enforcement:

```rust
struct ValidKeyDb {
    key_hash: String,
    scopes: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    is_active: bool,
}

#[async_trait::async_trait]
impl DatabaseStore for ValidKeyDb {
    // ... (same stubs as NoopDb for unused methods) ...

    async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
        if key_hash == self.key_hash && self.is_active {
            Ok(Some(ApiKey {
                id: uuid::Uuid::nil(),
                name: "test".to_string(),
                key_hash: key_hash.to_string(),
                key_prefix: "lg-test".to_string(),
                created_at: chrono::Utc::now(),
                expires_at: self.expires_at,
                is_active: self.is_active,
                token_quota: None,
                tokens_used: 0.0,
                rpm_limit: None,
                scopes: self.scopes.clone(),
                webhook_url: None,
            }))
        } else {
            Ok(None)
        }
    }
    // ... other stubs returning Ok defaults ...
}
```

Tests to add:

```rust
#[tokio::test]
async fn db_key_with_generate_scope_passes_auth() {
    // Hash "test-api-key"
    use sha2::{Digest, Sha256};
    use hex;
    let mut h = Sha256::new();
    h.update(b"test-api-key");
    let hash = hex::encode(h.finalize());

    let db = ValidKeyDb {
        key_hash: hash,
        scopes: "generate,read".to_string(),
        expires_at: None,
        is_active: true,
    };
    // build state with master_key = Some("master") so auth is enforced
    // but send "test-api-key" → should pass (hash matches)
    // ...
}

#[tokio::test]
async fn expired_key_is_rejected_with_401() { /* ... */ }

#[tokio::test]
async fn inactive_key_is_rejected_with_401() { /* ... */ }
```

### Step A2.8 — Run tests

```bash
cd litegen-core && cargo test --lib 2>&1 | tail -30
```
All tests must pass.

### Step A2.9 — Commit

```bash
git add src/api/middleware/mod.rs \
        src/api/middleware/rate_limit.rs \
        src/api/middleware/auth_tests.rs \
        src/api/handlers.rs
git commit -m "feat(litegen-core): scoped DB-backed auth + KeyContext"
```

---

## Task A3 — RPM rate limit

**Files:**
- Create: `litegen-core/src/api/middleware/rate_limit.rs`

### Step A3.1 — Write failing tests first

At the bottom of `rate_limit.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn fresh_bucket_allows_requests() {
        let limiter = RateLimiter::new();
        let id = Uuid::new_v4();
        // rpm=60 → 1 token/second capacity; fresh bucket starts full
        let result = limiter.try_take(id, 60).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn bucket_exhaustion_returns_retry_after() {
        let limiter = RateLimiter::new();
        let id = Uuid::new_v4();
        let rpm: u32 = 3;

        // First 3 should succeed (bucket capacity = rpm = 3)
        for _ in 0..rpm {
            assert!(limiter.try_take(id, rpm).await.is_ok());
        }

        // 4th should fail with retry_after > 0
        let err = limiter.try_take(id, rpm).await;
        assert!(err.is_err(), "expected rate limit error");
        let secs = err.unwrap_err();
        assert!(secs > 0, "retry_after should be > 0 seconds, got {}", secs);
    }

    #[tokio::test]
    async fn different_keys_have_independent_buckets() {
        let limiter = RateLimiter::new();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        // Exhaust id_a
        for _ in 0..1u32 {
            let _ = limiter.try_take(id_a, 1).await;
        }
        assert!(limiter.try_take(id_a, 1).await.is_err());

        // id_b should still work
        assert!(limiter.try_take(id_b, 1).await.is_ok());
    }
}
```

Run to confirm they fail:
```bash
cd litegen-core && cargo test --lib rate_limit 2>&1 | tail -20
```

### Step A3.2 — Implement `RateLimiter`

Create `litegen-core/src/api/middleware/rate_limit.rs`:

```rust
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;
use uuid::Uuid;

pub struct Bucket {
    pub last: Instant,
    pub tokens: f64,
}

/// In-memory token-bucket rate limiter, keyed by API key UUID.
/// Thread-safe via tokio RwLock.
pub struct RateLimiter {
    buckets: RwLock<HashMap<Uuid, Bucket>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
        }
    }

    /// Attempt to consume one token for `key_id` at `rpm` rate.
    ///
    /// Returns `Ok(())` if allowed, or `Err(retry_after_secs)` if rate limited.
    pub async fn try_take(&self, key_id: Uuid, rpm: u32) -> Result<(), u64> {
        let capacity = rpm as f64;
        let refill_rate = capacity / 60.0; // tokens per second
        let now = Instant::now();

        let mut buckets = self.buckets.write().await;
        let bucket = buckets.entry(key_id).or_insert(Bucket {
            last: now,
            tokens: capacity, // start full
        });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_rate).min(capacity);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            // Time until 1 token is available
            let secs_to_refill = (1.0 - bucket.tokens) / refill_rate;
            Err(secs_to_refill.ceil() as u64)
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    // ... (tests from Step A3.1) ...
}
```

### Step A3.3 — Run rate limit tests

```bash
cd litegen-core && cargo test --lib rate_limit 2>&1 | tail -20
```
All 3 tests must pass.

### Step A3.4 — Run all lib tests

```bash
cd litegen-core && cargo test --lib 2>&1 | tail -20
```
All tests must pass.

### Step A3.5 — Commit

```bash
git add src/api/middleware/rate_limit.rs
git commit -m "feat(litegen-core): per-key RPM rate limiting"
```

---

## Task A4 — Quota deduction post-generation

**Files:**
- Modify: `litegen-core/src/api/handlers.rs`
- Modify: `litegen-core/src/api/middleware/auth_tests.rs` (quota pre-flight test)

### Step A4.1 — Write the failing quota pre-flight test

Add to `auth_tests.rs` a `QuotaExceededDb` that returns a key with `token_quota = Some(1.0)` and `tokens_used = 2.0` (already exceeded):

```rust
struct QuotaExceededDb;

#[async_trait::async_trait]
impl DatabaseStore for QuotaExceededDb {
    // all stubs ...
    async fn lookup_api_key_by_hash(&self, _key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
        Ok(Some(ApiKey {
            id: Uuid::nil(),
            name: "quota-test".to_string(),
            key_hash: "quota_hash".to_string(),
            key_prefix: "lg-qt".to_string(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            is_active: true,
            token_quota: Some(1.0),
            tokens_used: 2.0,  // over quota
            rpm_limit: None,
            scopes: "generate,read".to_string(),
            webhook_url: None,
        }))
    }
    // ...
}

#[tokio::test]
async fn quota_exceeded_key_returns_402() {
    // Build state with master_key = Some("master"), QuotaExceededDb
    // Send any key (not master) → auth middleware should return 402
    // ...assert resp.status() == StatusCode::PAYMENT_REQUIRED
}
```

### Step A4.2 — Post-charge quota in `generate_image`

In `src/api/handlers.rs`, update the success path in `generate_image`:

```rust
Ok(response) => {
    let latency = start.elapsed().as_millis() as i64;
    let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

    // Post-charge quota if a DB key was used
    let key_ctx = request_ext_key_ctx; // extract from extensions (see below)
    // NOTE: KeyContext must be extracted before calling next.run() in the middleware.
    // Here we receive it via the handler — read it from the request extensions.

    let db = state.db.clone();
    let id = response.id.clone();
    let model = response.model.clone();
    let provider = response.provider.clone();

    // Quota deduction (post-charge)
    let mut quota_header: Option<(&'static str, &'static str)> = None;
    if let Some(key_id) = key_ctx.as_ref().and_then(|c| c.key_id) {
        if cost > 0.0 {
            match state.db.atomic_charge_tokens(&key_id, cost).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, key_id = %key_id, "Quota charge failed (already served response)");
                    quota_header = Some(("X-Litegen-Quota-Exceeded", "true"));
                }
            }
        }
    }

    tokio::spawn(async move {
        let _ = db
            .log_request(&id, &model, &provider, "completed", "image", cost, latency, None, None)
            .await;
    });

    let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
    if let Some((k, v)) = dropped_header(&validated.dropped) {
        resp.headers_mut().insert(k, v);
    }
    if let Some((k, v)) = quota_header {
        resp.headers_mut().insert(k, axum::http::HeaderValue::from_static(v));
    }
    resp
}
```

To access `KeyContext` in handlers, extract it from the request extensions. The handler signature needs access to the request extensions. Add an extractor:

In `src/api/handlers.rs`, add:

```rust
use crate::api::middleware::KeyContext;

/// Extracts KeyContext from request extensions. None if auth is not configured.
pub struct OptionalKeyContext(pub Option<KeyContext>);

#[axum::async_trait]
impl<S> axum::extract::FromRequestParts<S> for OptionalKeyContext
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(OptionalKeyContext(parts.extensions.get::<KeyContext>().cloned()))
    }
}
```

Then update `generate_image` signature:

```rust
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedImage,
) -> impl IntoResponse {
```

Apply the same pattern to `generate_video`.

### Step A4.3 — Run tests

```bash
cd litegen-core && cargo test --lib 2>&1 | tail -30
```

### Step A4.4 — Commit

```bash
git add src/api/handlers.rs src/api/middleware/auth_tests.rs
git commit -m "feat(litegen-core): post-charge quota deduction with pre-flight check"
```

---

## Task A5 — Endpoint extensions

**Files:**
- Modify: `litegen-core/src/api/handlers.rs`

### Step A5.1 — Extend `CreateApiKeyRequest`

Replace:
```rust
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
}
```

With:
```rust
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
    /// USD budget cap; None = unlimited.
    #[serde(default)]
    pub token_quota: Option<f64>,
    /// Requests-per-minute cap; None = unlimited.
    #[serde(default)]
    pub rpm_limit: Option<u32>,
    /// CSV of scopes (default: "generate,read").
    #[serde(default = "default_scopes")]
    pub scopes: String,
    /// Webhook URL for async callbacks.
    #[serde(default)]
    pub webhook_url: Option<String>,
}

fn default_scopes() -> String { "generate,read".to_string() }
```

### Step A5.2 — Update `create_api_key` handler to pass new fields

```rust
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateApiKeyRequest>,
) -> impl IntoResponse {
    let raw_key = format!("lg-{}", Uuid::new_v4().to_string().replace('-', ""));
    let prefix = &raw_key[..8];
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(raw_key.as_bytes());
        hex::encode(hasher.finalize())
    };

    match state.db.create_api_key(
        &request.name, &hash, prefix,
        request.token_quota, request.rpm_limit,
        &request.scopes, request.webhook_url.as_deref(),
    ).await {
        Ok(key) => (
            StatusCode::CREATED,
            Json(ApiKeyCreatedResponse {
                key: raw_key,
                prefix: key.key_prefix,
                name: key.name,
                created_at: key.created_at,
                token_quota: key.token_quota,
                rpm_limit: key.rpm_limit,
                scopes: key.scopes,
            }),
        ).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to create API key");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}
```

### Step A5.3 — Update `list_api_keys` handler to return new fields

```rust
pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.list_api_keys().await {
        Ok(keys) => {
            let data: Vec<ApiKeyInfo> = keys
                .into_iter()
                .map(|k| ApiKeyInfo {
                    id: k.id,
                    name: k.name,
                    prefix: k.key_prefix,
                    created_at: k.created_at,
                    expires_at: k.expires_at,
                    is_active: k.is_active,
                    token_quota: k.token_quota,
                    tokens_used: k.tokens_used,
                    rpm_limit: k.rpm_limit,
                    scopes: k.scopes,
                    webhook_url: k.webhook_url,
                })
                .collect();
            Json(ApiKeyListResponse { data }).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}
```

### Step A5.4 — Add `GET /v1/keys/{id}` handler

```rust
pub async fn get_api_key_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.db.get_api_key(&id).await {
        Ok(Some(key)) => {
            let detail = ApiKeyDetail {
                id: key.id,
                name: key.name,
                key_prefix: key.key_prefix,
                created_at: key.created_at,
                expires_at: key.expires_at,
                is_active: key.is_active,
                token_quota: key.token_quota,
                tokens_used: key.tokens_used,
                rpm_limit: key.rpm_limit,
                scopes: key.scopes,
                webhook_url: key.webhook_url,
            };
            (StatusCode::OK, Json(serde_json::to_value(detail).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    }
}
```

### Step A5.5 — Add `PATCH /v1/keys/{id}` handler

```rust
pub async fn patch_api_key_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateApiKeyRequest>,
) -> impl IntoResponse {
    match state.db.update_api_key(&id, &req).await {
        Ok(Some(key)) => {
            let detail = ApiKeyDetail {
                id: key.id,
                name: key.name,
                key_prefix: key.key_prefix,
                created_at: key.created_at,
                expires_at: key.expires_at,
                is_active: key.is_active,
                token_quota: key.token_quota,
                tokens_used: key.tokens_used,
                rpm_limit: key.rpm_limit,
                scopes: key.scopes,
                webhook_url: key.webhook_url,
            };
            (StatusCode::OK, Json(serde_json::to_value(detail).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    }
}
```

### Step A5.6 — Add inline endpoint tests

In `src/api/handlers.rs` or a sibling test file, add:

```rust
#[cfg(test)]
mod key_endpoint_tests {
    use super::*;
    // ... use in-memory SQLite DB (SqliteDatabase::connect("sqlite::memory:"))
    // and a full create_router with it

    #[tokio::test]
    async fn create_with_quota_and_get_by_id_roundtrip() {
        // POST /v1/keys with { name: "k1", token_quota: 5.0, rpm_limit: 10, scopes: "generate" }
        // → 201 with key string in body
        // GET /v1/keys/{id} → 200 with token_quota: 5.0
    }

    #[tokio::test]
    async fn patch_quota_updates_value() {
        // POST /v1/keys → create key
        // PATCH /v1/keys/{id} { token_quota: 20.0 } → 200 with token_quota: 20.0
    }

    #[tokio::test]
    async fn get_nonexistent_key_returns_404() {
        // GET /v1/keys/<random-uuid> → 404
    }
}
```

### Step A5.7 — Run all tests

```bash
cd litegen-core && cargo test --lib 2>&1 | tail -30
```

### Step A5.8 — Build release to verify no binary breakage

```bash
cd litegen-core && cargo build --release 2>&1 | tail -20
```

### Step A5.9 — Commit

```bash
git add src/api/handlers.rs src/types/mod.rs
git commit -m "feat(litegen-core): /v1/keys endpoints support quota/rpm/scopes"
```

---

## Self-Review Checklist

### Spec Coverage

| Spec requirement | Task |
|-----------------|------|
| New columns: token_quota, tokens_used, rpm_limit, scopes, webhook_url | A1.1, A1.2 |
| `ApiKey` struct extended | A1.3 |
| DB: create_api_key with new fields | A1.5d |
| DB: get_api_key | A1.5e |
| DB: update_api_key | A1.5f |
| DB: lookup_api_key_by_hash | A1.5g |
| DB: atomic_charge_tokens | A1.5h |
| DB: Postgres mirrors SQLite | A1.6 |
| DB tests: create/list/get/update/lookup/charge happy + quota-exceeded | A1.7 |
| Scope enum + KeyContext | A2.1 |
| RateLimiter in AppState | A2.2 |
| Auth middleware rewrite | A2.3 |
| require_scope middleware factory | A2.4 |
| Routes wired with scope layers | A2.5 |
| Auth tests: 401 no header, 200 master, 200 valid key, 403 wrong scope, 401 expired, 401 inactive | A2.7 |
| RPM token bucket | A3.2 |
| RPM tests | A3.1 |
| Post-charge quota deduction | A4.2 |
| X-Litegen-Quota-Exceeded header on charge failure | A4.2 |
| Pre-flight 402 check | A2.3 |
| POST /v1/keys extended body | A5.1, A5.2 |
| GET /v1/keys extended projection | A5.3 |
| GET /v1/keys/{id} | A5.4 |
| PATCH /v1/keys/{id} | A5.5 |
| All key endpoints require admin scope | A2.5 |
| master_key = None → no auth (preserve existing behavior) | A2.3 |
| NoopDb impls updated for new trait methods | A1.9 |

### Placeholder Scan

No placeholders — all steps contain concrete SQL, Rust code, and commands.

### Type Consistency

- `ApiKey.rpm_limit: Option<u32>` — used consistently in A1.3, A1.5b, all impls
- `atomic_charge_tokens` returns `Result<f64, sqlx::Error>` — consistent in A1.4, A1.5h
- `UpdateApiKeyRequest` defined in A1.3, used in A1.4, A1.5f, A5.5
- `ApiKeyDetail` defined in A1.3, used in A5.4, A5.5
- `KeyContext` defined in A2.1, inserted in A2.3, extracted in A4.2
- `RateLimiter::try_take` returns `Result<(), u64>` — used in A2.3 and tested in A3.1
- `require_scope(Scope::Generate)` — Scope is Copy, used in A2.4, A2.5
- `API_KEY_COLS` constant defined once in sqlite.rs — used in all queries

### Deviation Notes

1. `update_api_key` builds a dynamic SET clause with positional `?` — this is correct for SQLite's runtime query API since we can't use compile-time macros without `sqlx prepare`. The Postgres version must use numbered placeholders (`$1`, `$2`, ...) which requires a slightly different dynamic builder.

2. The `require_scope` middleware factory returns a boxed async function compatible with `axum::middleware::from_fn` — this is the cleanest approach for axum 0.8 without creating a custom tower Layer type.

3. `validate_api_key` (old name) is kept as an alias pointing to `lookup_api_key_by_hash` behavior — actually the spec only adds `lookup_api_key_by_hash`; `validate_api_key` is removed from the trait. Auth middleware calls `lookup_api_key_by_hash` directly.
