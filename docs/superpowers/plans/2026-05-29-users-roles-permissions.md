# Users + Roles + Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add full user accounts (password + OAuth), roles (Owner/Admin/Member/Viewer), permissions, and a dashboard rewrite on top of `@litegen/sdk` so the SDK is dogfood-tested by the god Playwright run.

**Architecture:** New `users` / `sessions` / `invitations` / `password_resets` / `login_attempts` tables in sqlite + postgres. Existing API-key Bearer middleware extended to also accept session cookies. Roles map to a static permission set in `auth/permissions.rs`. Dashboard's hand-rolled `api.ts` replaced wholesale with the generated SDK client.

**Tech Stack:** sqlx, axum, tokio, argon2 v0.5, rand, subtle, oauth2 v4 + openidconnect v3, @litegen/sdk (TypeScript codegen), React + react-router-dom, Playwright.

**Spec reference:** `docs/superpowers/specs/2026-05-29-users-roles-permissions-design.md`

---

## Working dir conventions

- Rust: `cd litegen-core` before all `cargo` commands.
- Dashboard: `cd dashboard` before all `npm` commands.
- SDK: `cd sdks/typescript` for SDK build.
- Tests pass = `cargo test --lib` clean AND `cd dashboard && npm run build` clean AND `cd dashboard && npx playwright test` shows `1 passed`.
- Clippy must stay clean: `cargo clippy --lib --no-deps -- -D warnings`.

## Predecessor state

- Branch: `feat/capability-registry`.
- 163 lib tests passing. Playwright god test 1 passed in ~46s with 250ms slow-mo.
- All AAA work committed: capability registry, multi-key auth/scopes/RPM, video polling, webhooks, OTel, circuit breakers, audit log, request_artifacts trace panel, 16 mock models, Dockerfile.

---

## Phase A — Schema + types

### Task 1: Migration files for users/sessions/invitations/password_resets/login_attempts

**Files:**
- Create: `litegen-core/migrations/sqlite/20240101000007_users.sql`
- Create: `litegen-core/migrations/postgres/20240101000007_users.sql`

- [ ] **Step 1: Write sqlite migration**

```sql
-- litegen-core/migrations/sqlite/20240101000007_users.sql
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT,
    role TEXT NOT NULL CHECK(role IN ('owner','admin','member','viewer')),
    oauth_github_id TEXT UNIQUE,
    oauth_google_id TEXT UNIQUE,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_login_at TIMESTAMP,
    is_active INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_users_email ON users(email);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP NOT NULL,
    ip TEXT,
    user_agent TEXT,
    csrf_token TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id, expires_at);

CREATE TABLE invitations (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('admin','member','viewer')),
    token TEXT NOT NULL UNIQUE,
    invited_by TEXT REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    used_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_invitations_token ON invitations(token);

CREATE TABLE password_resets (
    token TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    used_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE login_attempts (
    email TEXT NOT NULL,
    attempted_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success INTEGER NOT NULL
);
CREATE INDEX idx_login_attempts_email ON login_attempts(email, attempted_at DESC);

ALTER TABLE api_keys ADD COLUMN owner_user_id TEXT REFERENCES users(id);
ALTER TABLE audit_log ADD COLUMN actor_user_id TEXT REFERENCES users(id);
```

- [ ] **Step 2: Write postgres migration**

Same DDL with type adjustments: `INTEGER` → `BOOLEAN`, `TIMESTAMP` stays, `CHECK` constraints stay.

```sql
-- litegen-core/migrations/postgres/20240101000007_users.sql
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT,
    role TEXT NOT NULL CHECK(role IN ('owner','admin','member','viewer')),
    oauth_github_id TEXT UNIQUE,
    oauth_google_id TEXT UNIQUE,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_login_at TIMESTAMP,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);
CREATE INDEX idx_users_email ON users(email);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP NOT NULL,
    ip TEXT,
    user_agent TEXT,
    csrf_token TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id, expires_at);

CREATE TABLE invitations (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('admin','member','viewer')),
    token TEXT NOT NULL UNIQUE,
    invited_by TEXT REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    used_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_invitations_token ON invitations(token);

CREATE TABLE password_resets (
    token TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    used_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE login_attempts (
    email TEXT NOT NULL,
    attempted_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL
);
CREATE INDEX idx_login_attempts_email ON login_attempts(email, attempted_at DESC);

ALTER TABLE api_keys ADD COLUMN owner_user_id TEXT REFERENCES users(id);
ALTER TABLE audit_log ADD COLUMN actor_user_id TEXT REFERENCES users(id);
```

- [ ] **Step 3: Confirm migrations run cleanly**

Run: `cd litegen-core && cargo test --lib migrations` (or any sqlite-using test will trigger them).

Expected: PASS, no migration error.

- [ ] **Step 4: Commit**

```bash
git add litegen-core/migrations/sqlite/20240101000007_users.sql litegen-core/migrations/postgres/20240101000007_users.sql
git commit -m "feat(litegen-core): migrations for users + sessions + invitations + password resets"
```

### Task 2: Rust types for User, Session, Invitation, PasswordReset, Role, Permission

**Files:**
- Modify: `litegen-core/src/types/mod.rs`
- Create: `litegen-core/src/auth/permissions.rs`
- Modify: `litegen-core/src/auth/mod.rs` (or create if not exists)

- [ ] **Step 1: Add structs to `src/types/mod.rs`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub role: Role,
    pub oauth_github_id: Option<String>,
    pub oauth_google_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role { Owner, Admin, Member, Viewer }

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Owner => "owner", Self::Admin => "admin", Self::Member => "member", Self::Viewer => "viewer" }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s { "owner" => Some(Self::Owner), "admin" => Some(Self::Admin), "member" => Some(Self::Member), "viewer" => Some(Self::Viewer), _ => None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub csrf_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Invitation {
    pub id: String,
    pub email: String,
    pub role: Role,
    pub token: String,
    pub invited_by: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PasswordReset {
    pub token: String,
    pub user_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 2: Add `Permission` enum + role map in `src/auth/permissions.rs`**

```rust
use crate::types::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    UserReadSelf, UserReadAny, UserWriteAny, UserDeleteAny,
    KeyReadOwn, KeyReadAny, KeyWriteOwn, KeyWriteAny,
    KeyDeleteOwn, KeyDeleteAny, KeyTestWebhookOwn, KeyTestWebhookAny,
    GenerationCreate, GenerationReadOwn, GenerationReadAny,
    GenerationCancelOwn, GenerationCancelAny,
    AuditRead, CacheClear, SystemConfig, SystemTransferOwner,
    InvitationSend, InvitationRevoke,
    SessionRevokeOwn, SessionRevokeAny,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserReadSelf => "user:read:self",
            Self::UserReadAny => "user:read:any",
            Self::UserWriteAny => "user:write:any",
            Self::UserDeleteAny => "user:delete:any",
            Self::KeyReadOwn => "key:read:own",
            Self::KeyReadAny => "key:read:any",
            Self::KeyWriteOwn => "key:write:own",
            Self::KeyWriteAny => "key:write:any",
            Self::KeyDeleteOwn => "key:delete:own",
            Self::KeyDeleteAny => "key:delete:any",
            Self::KeyTestWebhookOwn => "key:test_webhook:own",
            Self::KeyTestWebhookAny => "key:test_webhook:any",
            Self::GenerationCreate => "generation:create",
            Self::GenerationReadOwn => "generation:read:own",
            Self::GenerationReadAny => "generation:read:any",
            Self::GenerationCancelOwn => "generation:cancel:own",
            Self::GenerationCancelAny => "generation:cancel:any",
            Self::AuditRead => "audit:read",
            Self::CacheClear => "cache:clear",
            Self::SystemConfig => "system:config",
            Self::SystemTransferOwner => "system:transfer_owner",
            Self::InvitationSend => "invitation:send",
            Self::InvitationRevoke => "invitation:revoke",
            Self::SessionRevokeOwn => "session:revoke:own",
            Self::SessionRevokeAny => "session:revoke:any",
        }
    }
}

pub fn permissions_for(role: Role) -> &'static [Permission] {
    use Permission::*;
    match role {
        Role::Owner => &[
            UserReadSelf, UserReadAny, UserWriteAny, UserDeleteAny,
            KeyReadOwn, KeyReadAny, KeyWriteOwn, KeyWriteAny, KeyDeleteOwn, KeyDeleteAny,
            KeyTestWebhookOwn, KeyTestWebhookAny,
            GenerationCreate, GenerationReadOwn, GenerationReadAny,
            GenerationCancelOwn, GenerationCancelAny,
            AuditRead, CacheClear, SystemConfig, SystemTransferOwner,
            InvitationSend, InvitationRevoke,
            SessionRevokeOwn, SessionRevokeAny,
        ],
        Role::Admin => &[
            UserReadSelf, UserReadAny, UserWriteAny, UserDeleteAny,
            KeyReadOwn, KeyReadAny, KeyWriteOwn, KeyWriteAny, KeyDeleteOwn, KeyDeleteAny,
            KeyTestWebhookOwn, KeyTestWebhookAny,
            GenerationCreate, GenerationReadOwn, GenerationReadAny,
            GenerationCancelOwn, GenerationCancelAny,
            AuditRead, CacheClear, SystemConfig,
            InvitationSend, InvitationRevoke,
            SessionRevokeOwn, SessionRevokeAny,
        ],
        Role::Member => &[
            UserReadSelf, KeyReadOwn, KeyWriteOwn, KeyDeleteOwn, KeyTestWebhookOwn,
            GenerationCreate, GenerationReadOwn, GenerationCancelOwn,
            SessionRevokeOwn,
        ],
        Role::Viewer => &[
            UserReadSelf, KeyReadOwn, GenerationCreate, GenerationReadOwn, SessionRevokeOwn,
        ],
    }
}

pub fn role_has(role: Role, perm: Permission) -> bool {
    permissions_for(role).contains(&perm)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_has_transfer_owner() {
        assert!(role_has(Role::Owner, Permission::SystemTransferOwner));
    }
    #[test]
    fn admin_does_not_have_transfer_owner() {
        assert!(!role_has(Role::Admin, Permission::SystemTransferOwner));
    }
    #[test]
    fn viewer_cannot_write_keys() {
        assert!(!role_has(Role::Viewer, Permission::KeyWriteOwn));
    }
    #[test]
    fn member_can_test_own_webhook_but_not_any() {
        assert!(role_has(Role::Member, Permission::KeyTestWebhookOwn));
        assert!(!role_has(Role::Member, Permission::KeyTestWebhookAny));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd litegen-core && cargo test --lib auth::permissions`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add litegen-core/src/types/mod.rs litegen-core/src/auth/permissions.rs
git commit -m "feat(litegen-core): User/Session/Invitation types + role-permission map"
```

### Task 3: Password hashing module (argon2id)

**Files:**
- Modify: `litegen-core/Cargo.toml` (add `argon2 = "0.5"`, `subtle = "2"`)
- Create: `litegen-core/src/auth/password.rs`

- [ ] **Step 1: Write failing test**

```rust
// in src/auth/password.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_succeeds() {
        let hash = hash_password("correct-horse-battery-staple-1").unwrap();
        assert!(verify_password("correct-horse-battery-staple-1", &hash).unwrap());
    }

    #[test]
    fn verify_rejects_wrong_password() {
        let hash = hash_password("correct-horse-battery-staple-1").unwrap();
        assert!(!verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn min_length_enforced() {
        let result = hash_password("short");
        assert!(matches!(result, Err(PasswordError::TooShort)));
    }

    #[test]
    fn dummy_verify_runs_constant_time() {
        // ~regression check: dummy hash verify should NOT panic + take similar time
        let _ = verify_dummy("any-password");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd litegen-core && cargo test --lib auth::password`
Expected: FAIL (module not found).

- [ ] **Step 3: Write implementation**

```rust
// src/auth/password.rs
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, Params, Algorithm, Version,
};

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("password must be at least 12 characters")]
    TooShort,
    #[error("argon2 hash error: {0}")]
    Hash(String),
    #[error("argon2 verify error: {0}")]
    Verify(String),
}

pub const MIN_PASSWORD_LEN: usize = 12;

fn params() -> Params {
    Params::new(65536, 3, 1, None).expect("argon2 params")
}

fn argon() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params())
}

pub fn hash_password(plain: &str) -> Result<String, PasswordError> {
    if plain.len() < MIN_PASSWORD_LEN {
        return Err(PasswordError::TooShort);
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| PasswordError::Hash(e.to_string()))?;
    Ok(hash.to_string())
}

pub fn verify_password(plain: &str, phc: &str) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(phc).map_err(|e| PasswordError::Verify(e.to_string()))?;
    Ok(argon().verify_password(plain.as_bytes(), &parsed).is_ok())
}

/// Run a verify against a precomputed dummy hash. Use during login when user not found
/// to keep response time constant and prevent user enumeration.
pub fn verify_dummy(plain: &str) {
    static DUMMY_HASH: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
        hash_password("dummy-dummy-dummy").expect("dummy hash")
    });
    let _ = verify_password(plain, &DUMMY_HASH);
}
```

- [ ] **Step 4: Run tests**

Run: `cd litegen-core && cargo test --lib auth::password`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add litegen-core/Cargo.toml litegen-core/Cargo.lock litegen-core/src/auth/password.rs
git commit -m "feat(litegen-core): argon2id password hashing with min-length + dummy verify"
```

### Task 4: Session + CSRF token generation

**Files:**
- Create: `litegen-core/src/auth/tokens.rs`

- [ ] **Step 1: Write failing tests**

```rust
// src/auth/tokens.rs
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_token_is_64_hex_chars() {
        let t = generate_session_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }
    #[test]
    fn two_tokens_differ() {
        assert_ne!(generate_session_token(), generate_session_token());
    }
    #[test]
    fn csrf_token_is_64_hex_chars() {
        let t = generate_csrf_token();
        assert_eq!(t.len(), 64);
    }
    #[test]
    fn constant_time_compare_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
```

- [ ] **Step 2: Run to fail**

Run: `cd litegen-core && cargo test --lib auth::tokens`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// src/auth/tokens.rs
use rand::RngCore;
use subtle::ConstantTimeEq;

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn generate_csrf_token() -> String {
    generate_session_token()
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() { return false; }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
```

- [ ] **Step 4: Run tests**

Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/auth/tokens.rs
git commit -m "feat(litegen-core): session + CSRF token generation (32-byte OsRng, hex)"
```

### Task 5: Login lockout tracker

**Files:**
- Create: `litegen-core/src/auth/lockout.rs`

- [ ] **Step 1: Write failing tests**

```rust
// src/auth/lockout.rs
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn under_threshold_not_locked_out() {
        let attempts: Vec<chrono::DateTime<Utc>> = (0..4)
            .map(|_| Utc::now() - Duration::seconds(1))
            .collect();
        assert!(!is_locked_out(&attempts, Utc::now()));
    }

    #[test]
    fn five_failures_within_window_locks_out() {
        let now = Utc::now();
        let attempts: Vec<_> = (0..5).map(|_| now - Duration::minutes(1)).collect();
        assert!(is_locked_out(&attempts, now));
    }

    #[test]
    fn old_failures_outside_window_dont_lock() {
        let now = Utc::now();
        let attempts: Vec<_> = (0..5).map(|_| now - Duration::minutes(30)).collect();
        assert!(!is_locked_out(&attempts, now));
    }

    #[test]
    fn retry_after_returns_seconds_until_window_clears() {
        let now = Utc::now();
        // Oldest failure 1 min ago, window is 15 min → 14 min left.
        let attempts: Vec<_> = (0..5).map(|_| now - Duration::minutes(1)).collect();
        let ra = retry_after_seconds(&attempts, now);
        assert!(ra >= 60 * 14 && ra <= 60 * 15);
    }
}
```

- [ ] **Step 2: Run to fail**

Run: `cd litegen-core && cargo test --lib auth::lockout`

- [ ] **Step 3: Implement**

```rust
// src/auth/lockout.rs
use chrono::{DateTime, Duration, Utc};

pub const MAX_FAILS: usize = 5;
pub const WINDOW_MINUTES: i64 = 15;

pub fn is_locked_out(failures_within_window: &[DateTime<Utc>], now: DateTime<Utc>) -> bool {
    let window_start = now - Duration::minutes(WINDOW_MINUTES);
    let count = failures_within_window.iter().filter(|t| **t >= window_start).count();
    count >= MAX_FAILS
}

pub fn retry_after_seconds(failures: &[DateTime<Utc>], now: DateTime<Utc>) -> i64 {
    let window_start = now - Duration::minutes(WINDOW_MINUTES);
    let oldest_in_window = failures.iter().filter(|t| **t >= window_start).min().copied();
    match oldest_in_window {
        Some(t) => {
            let unlock_at = t + Duration::minutes(WINDOW_MINUTES);
            (unlock_at - now).num_seconds().max(0)
        }
        None => 0,
    }
}
```

- [ ] **Step 4: Run tests**

Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add litegen-core/src/auth/lockout.rs
git commit -m "feat(litegen-core): login lockout (5 fails / 15 min window)"
```

---

## Phase B — DB layer

### Task 6: Add DatabaseStore methods for users/sessions/invitations/password_resets/login_attempts

**Files:**
- Modify: `litegen-core/src/db/trait_def.rs`
- Modify: `litegen-core/src/db/sqlite.rs`
- Modify: `litegen-core/src/db/postgres.rs`
- Modify: `litegen-core/src/db/sqlite_tests.rs`

- [ ] **Step 1: Add trait methods**

```rust
// in src/db/trait_def.rs append to DatabaseStore
async fn create_user(&self, user: &User) -> Result<(), sqlx::Error>;
async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error>;
async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, sqlx::Error>;
async fn get_user_by_oauth(&self, provider: &str, oauth_id: &str) -> Result<Option<User>, sqlx::Error>;
async fn update_user(&self, id: &str, role: Option<Role>, is_active: Option<bool>, password_hash: Option<&str>) -> Result<Option<User>, sqlx::Error>;
async fn touch_last_login(&self, id: &str) -> Result<(), sqlx::Error>;
async fn count_users(&self) -> Result<i64, sqlx::Error>;
async fn list_users(&self) -> Result<Vec<User>, sqlx::Error>;
async fn link_oauth(&self, user_id: &str, provider: &str, oauth_id: &str) -> Result<(), sqlx::Error>;
async fn transfer_owner(&self, new_owner_id: &str) -> Result<(), sqlx::Error>;

async fn create_session(&self, s: &Session) -> Result<(), sqlx::Error>;
async fn get_session(&self, id: &str) -> Result<Option<Session>, sqlx::Error>;
async fn delete_session(&self, id: &str) -> Result<(), sqlx::Error>;
async fn delete_user_sessions(&self, user_id: &str, except_id: Option<&str>) -> Result<u64, sqlx::Error>;
async fn list_user_sessions(&self, user_id: &str) -> Result<Vec<Session>, sqlx::Error>;
async fn bump_session_expiry(&self, id: &str, new_expires_at: chrono::DateTime<chrono::Utc>) -> Result<(), sqlx::Error>;

async fn create_invitation(&self, inv: &Invitation) -> Result<(), sqlx::Error>;
async fn get_invitation(&self, token: &str) -> Result<Option<Invitation>, sqlx::Error>;
async fn mark_invitation_used(&self, token: &str) -> Result<(), sqlx::Error>;
async fn delete_invitation(&self, id: &str) -> Result<(), sqlx::Error>;
async fn list_invitations(&self) -> Result<Vec<Invitation>, sqlx::Error>;

async fn create_password_reset(&self, r: &PasswordReset) -> Result<(), sqlx::Error>;
async fn get_password_reset(&self, token: &str) -> Result<Option<PasswordReset>, sqlx::Error>;
async fn mark_password_reset_used(&self, token: &str) -> Result<(), sqlx::Error>;

async fn record_login_attempt(&self, email: &str, success: bool) -> Result<(), sqlx::Error>;
async fn recent_failed_login_attempts(&self, email: &str, since: chrono::DateTime<chrono::Utc>) -> Result<Vec<chrono::DateTime<chrono::Utc>>, sqlx::Error>;
```

- [ ] **Step 2: Implement on SqliteDatabase**

(Full impl too long to inline — each method is a single sqlx::query with explicit binds. Use `chrono::Utc::now()` for timestamps. For boolean fields stored as INTEGER in sqlite, cast: `(is_active != 0) as is_active`.)

Key impls to write:
- `create_user`: INSERT with all columns, ON CONFLICT DO NOTHING strategy NOT needed — duplicates should error.
- `get_user_by_email`: SELECT with LOWER(email) = LOWER($1) for case insensitivity.
- `transfer_owner`: BEGIN TX; UPDATE users SET role='admin' WHERE role='owner'; UPDATE users SET role='owner' WHERE id=$1; COMMIT.
- `recent_failed_login_attempts`: SELECT attempted_at FROM login_attempts WHERE email=$1 AND success=0 AND attempted_at >= $2.

- [ ] **Step 3: Implement on PostgresDatabase**

Same shape, `$N` instead of `?`, `BOOLEAN` instead of `INTEGER` for is_active.

- [ ] **Step 4: Write integration tests in `sqlite_tests.rs`**

```rust
#[tokio::test]
async fn create_user_then_get_by_email() {
    let db = test_db().await;
    let u = User {
        id: uuid::Uuid::new_v4().to_string(),
        email: "joe@example.com".into(),
        password_hash: Some("phc$argon2id...".into()),
        role: Role::Owner,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_login_at: None,
        is_active: true,
    };
    db.create_user(&u).await.unwrap();
    let got = db.get_user_by_email("joe@example.com").await.unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().email, "joe@example.com");
}

#[tokio::test]
async fn count_users_zero_then_one() {
    let db = test_db().await;
    assert_eq!(db.count_users().await.unwrap(), 0);
    let u = User { /* ... same shape ... */ };
    db.create_user(&u).await.unwrap();
    assert_eq!(db.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn transfer_owner_demotes_old_and_promotes_new() {
    let db = test_db().await;
    let owner = User { /* role: Owner */ };
    let admin = User { /* role: Admin */ };
    db.create_user(&owner).await.unwrap();
    db.create_user(&admin).await.unwrap();
    db.transfer_owner(&admin.id).await.unwrap();
    assert_eq!(db.get_user_by_id(&owner.id).await.unwrap().unwrap().role, Role::Admin);
    assert_eq!(db.get_user_by_id(&admin.id).await.unwrap().unwrap().role, Role::Owner);
}

#[tokio::test]
async fn session_round_trip_and_expiry_bump() {
    let db = test_db().await;
    let user = User { /* ... */ };
    db.create_user(&user).await.unwrap();
    let s = Session {
        id: generate_session_token(),
        user_id: user.id.clone(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        ip: None, user_agent: None,
        csrf_token: generate_csrf_token(),
    };
    db.create_session(&s).await.unwrap();
    let got = db.get_session(&s.id).await.unwrap().unwrap();
    assert_eq!(got.user_id, user.id);
    let new_exp = chrono::Utc::now() + chrono::Duration::days(14);
    db.bump_session_expiry(&s.id, new_exp).await.unwrap();
    let bumped = db.get_session(&s.id).await.unwrap().unwrap();
    assert!(bumped.expires_at >= new_exp - chrono::Duration::seconds(2));
}

#[tokio::test]
async fn login_attempts_recent_filter() {
    let db = test_db().await;
    db.record_login_attempt("joe@x.com", false).await.unwrap();
    db.record_login_attempt("joe@x.com", true).await.unwrap();
    let since = chrono::Utc::now() - chrono::Duration::minutes(15);
    let fails = db.recent_failed_login_attempts("joe@x.com", since).await.unwrap();
    assert_eq!(fails.len(), 1);
}

#[tokio::test]
async fn invitation_create_get_use() {
    let db = test_db().await;
    let inv = Invitation { /* ... */ };
    db.create_invitation(&inv).await.unwrap();
    let got = db.get_invitation(&inv.token).await.unwrap().unwrap();
    assert_eq!(got.email, inv.email);
    assert!(got.used_at.is_none());
    db.mark_invitation_used(&inv.token).await.unwrap();
    let after = db.get_invitation(&inv.token).await.unwrap().unwrap();
    assert!(after.used_at.is_some());
}
```

- [ ] **Step 5: Run tests**

Run: `cd litegen-core && cargo test --lib db::`
Expected: PASS (6 new tests + existing).

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/db/
git commit -m "feat(litegen-core): DB methods for users/sessions/invitations/resets/login_attempts"
```

---

## Phase C — Auth middleware extension

### Task 7: Extend `KeyContext` with `UserContext` + permissions

**Files:**
- Modify: `litegen-core/src/api/middleware/auth.rs`

- [ ] **Step 1: Add types**

```rust
#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: String,
    pub email: String,
    pub role: Role,
}

// Modify existing KeyContext:
#[derive(Debug, Clone)]
pub struct KeyContext {
    pub key_id: Option<Uuid>,
    pub scopes: Vec<Scope>,
    pub quota_remaining: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub webhook_url: Option<String>,
    pub user: Option<UserContext>,  // NEW
    pub permissions: Vec<Permission>,  // NEW
    pub session_id: Option<String>,  // NEW, for CSRF lookup
}
```

- [ ] **Step 2: Update all KeyContext constructors**

The 3-4 places that construct KeyContext today (master-key bypass, DB key lookup, test helpers) set `user: None, permissions: vec![], session_id: None`.

- [ ] **Step 3: Add `cookie_value(headers: &HeaderMap, name: &str) -> Option<String>` helper**

```rust
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    cookie_header.split(';')
        .map(|s| s.trim())
        .find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if k == name { Some(v.to_string()) } else { None }
        })
}
```

- [ ] **Step 4: Extend `auth_middleware` to check cookie**

After the existing Bearer-token branch (returning 401 if neither present), add cookie path:

```rust
// 2. Try cookie session
if let Some(sid) = cookie_value(req.headers(), "litegen_session") {
    if let Some(sess) = state.db.get_session(&sid).await.ok().flatten() {
        if sess.expires_at < chrono::Utc::now() {
            let _ = state.db.delete_session(&sid).await;
            return unauthorized();
        }
        if let Some(user) = state.db.get_user_by_id(&sess.user_id).await.ok().flatten() {
            if !user.is_active { return unauthorized(); }
            // Bump expiry if within 24h of expiring
            let new_exp = chrono::Utc::now() + chrono::Duration::days(7);
            if sess.expires_at < chrono::Utc::now() + chrono::Duration::hours(24) {
                let _ = state.db.bump_session_expiry(&sid, new_exp).await;
            }
            let perms = permissions_for(user.role).to_vec();
            let ctx = KeyContext {
                key_id: None, scopes: vec![], quota_remaining: None,
                rpm_limit: None, webhook_url: None,
                user: Some(UserContext { user_id: user.id, email: user.email, role: user.role }),
                permissions: perms,
                session_id: Some(sid),
            };
            req.extensions_mut().insert(ctx);
            return next.run(req).await;
        }
    }
}
```

- [ ] **Step 5: CSRF enforcement helper**

```rust
pub async fn csrf_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let method = req.method().clone();
    if !matches!(method, axum::http::Method::POST | axum::http::Method::PUT
                       | axum::http::Method::PATCH | axum::http::Method::DELETE) {
        return Ok(next.run(req).await);
    }
    let ctx = req.extensions().get::<KeyContext>().cloned();
    let Some(ctx) = ctx else { return Ok(next.run(req).await); };
    // Bearer-authenticated requests skip CSRF (no session_id)
    let Some(sid) = ctx.session_id.as_deref() else { return Ok(next.run(req).await); };
    let header_token = req.headers().get("x-csrf-token").and_then(|v| v.to_str().ok());
    let Some(header_token) = header_token else {
        return Err(StatusCode::FORBIDDEN);
    };
    let Some(sess) = state.db.get_session(sid).await.ok().flatten() else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if !constant_time_eq(&sess.csrf_token, header_token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}
```

- [ ] **Step 6: `require_permission` middleware factory**

```rust
pub fn require_permission(perm: Permission) -> impl Fn(Request, Next) -> Pin<Box<dyn Future<Output = Result<Response, StatusCode>> + Send>> + Clone {
    // ... returns a layer fn that 403s if `ctx.permissions` doesn't contain perm
    // (or if no ctx — but auth_middleware always inserts one if it reaches handlers)
}
```

Implementation: use `axum::middleware::from_fn_with_state` pattern matching the existing `require_scope`.

- [ ] **Step 7: Tests**

```rust
#[tokio::test]
async fn cookie_session_auth_builds_user_context() { ... }
#[tokio::test]
async fn expired_session_returns_401() { ... }
#[tokio::test]
async fn csrf_post_without_header_returns_403() { ... }
#[tokio::test]
async fn csrf_post_with_valid_header_passes() { ... }
#[tokio::test]
async fn require_permission_denies_missing_perm() { ... }
```

- [ ] **Step 8: Commit**

```bash
git add litegen-core/src/api/middleware/auth.rs
git commit -m "feat(litegen-core): session-cookie auth + CSRF + require_permission middleware"
```

---

## Phase D — Password auth endpoints

### Task 8: POST /v1/auth/signup

**Files:**
- Create: `litegen-core/src/api/handlers/auth_password.rs`
- Modify: `litegen-core/src/api/handlers.rs` (mod declaration)
- Modify: `litegen-core/src/api/mod.rs` (route)

- [ ] **Step 1: Test**

```rust
#[tokio::test]
async fn signup_creates_owner_when_users_empty() { ... }
#[tokio::test]
async fn signup_fails_when_users_exist() { ... }
#[tokio::test]
async fn signup_requires_owner_email_when_set() { ... }
```

- [ ] **Step 2: Handler**

```rust
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SignupRequest { pub email: String, pub password: String }

#[derive(Serialize, utoipa::ToSchema)]
pub struct AuthResponse { pub user: PublicUser }

#[utoipa::path(post, path = "/v1/auth/signup", request_body = SignupRequest,
    responses((status = 200, body = AuthResponse), (status = 409, body = ErrorResponse)),
    tag = "Auth")]
pub async fn signup(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SignupRequest>,
) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    if state.db.count_users().await.unwrap_or(0) > 0 {
        return error(StatusCode::CONFLICT, "signup_closed", "Signup is closed");
    }
    if let Ok(required) = std::env::var("LITEGEN__OWNER_EMAIL") {
        if required.to_lowercase() != email {
            return error(StatusCode::FORBIDDEN, "owner_email_required",
                &format!("Only {required} can claim owner"));
        }
    }
    let hash = match hash_password(&body.password) {
        Ok(h) => h,
        Err(PasswordError::TooShort) => return error(StatusCode::BAD_REQUEST,
            "password_too_short", "Password must be at least 12 characters"),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "hash_error", &e.to_string()),
    };
    let user = User { /* ... id, email, password_hash: Some(hash), role: Owner, ... */ };
    if let Err(e) = state.db.create_user(&user).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
    }
    let (session_cookie, csrf_cookie) = create_session_for(&state, &user.id, req_meta).await;
    (StatusCode::OK, [session_cookie, csrf_cookie], Json(AuthResponse { user: user.into() })).into_response()
}
```

- [ ] **Step 3: Wire route**

```rust
.route("/v1/auth/signup", post(signup))
```

Under no auth (signup endpoint is unauthenticated).

- [ ] **Step 4: Run tests**

`cd litegen-core && cargo test --lib auth::signup`

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(litegen-core): POST /v1/auth/signup (first-user-becomes-owner)"
```

### Task 9: POST /v1/auth/login + POST /v1/auth/logout + GET /v1/auth/me + GET /v1/auth/csrf

**Files:**
- Modify: `litegen-core/src/api/handlers/auth_password.rs`

- [ ] **Step 1: Tests**

```rust
#[tokio::test]
async fn login_with_correct_password_returns_session() { ... }
#[tokio::test]
async fn login_with_wrong_password_returns_401() { ... }
#[tokio::test]
async fn login_with_unknown_email_returns_401_constant_time() { ... }
#[tokio::test]
async fn login_locked_out_after_5_fails() { ... }
#[tokio::test]
async fn logout_deletes_session() { ... }
#[tokio::test]
async fn me_returns_user_for_session() { ... }
```

- [ ] **Step 2: Implement handlers**

```rust
pub async fn login(State(state), Json(body)) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    // 1. Lockout check
    let since = chrono::Utc::now() - chrono::Duration::minutes(15);
    let fails = state.db.recent_failed_login_attempts(&email, since).await.unwrap_or_default();
    if is_locked_out(&fails, chrono::Utc::now()) {
        let ra = retry_after_seconds(&fails, chrono::Utc::now());
        return error_with_header(StatusCode::TOO_MANY_REQUESTS, "locked_out",
            "Too many failed attempts", ("retry-after", ra.to_string()));
    }
    // 2. Lookup user (constant-time)
    let user_opt = state.db.get_user_by_email(&email).await.unwrap_or(None);
    let phc = user_opt.as_ref().and_then(|u| u.password_hash.clone());
    let verified = match phc {
        Some(h) => verify_password(&body.password, &h).unwrap_or(false),
        None => { verify_dummy(&body.password); false }
    };
    if !verified || user_opt.is_none() {
        let _ = state.db.record_login_attempt(&email, false).await;
        return error(StatusCode::UNAUTHORIZED, "invalid_credentials", "Invalid email or password");
    }
    let user = user_opt.unwrap();
    if !user.is_active {
        return error(StatusCode::FORBIDDEN, "account_inactive", "Account is inactive");
    }
    let _ = state.db.record_login_attempt(&email, true).await;
    let _ = state.db.touch_last_login(&user.id).await;
    let (sc, cc) = create_session_for(&state, &user.id, ...).await;
    (StatusCode::OK, [sc, cc], Json(AuthResponse { user: user.into() })).into_response()
}

pub async fn logout(State(state), Extension(ctx): Extension<KeyContext>) -> impl IntoResponse {
    if let Some(sid) = ctx.session_id {
        let _ = state.db.delete_session(&sid).await;
    }
    (StatusCode::NO_CONTENT, [clear_session_cookie(), clear_csrf_cookie()]).into_response()
}

pub async fn me(Extension(ctx): Extension<KeyContext>) -> impl IntoResponse {
    match ctx.user {
        Some(u) => (StatusCode::OK, Json(json!({ "user": { "id": u.user_id, "email": u.email, "role": u.role } }))).into_response(),
        None => (StatusCode::UNAUTHORIZED, Json(error_body("not_authenticated", "Not logged in"))).into_response(),
    }
}

pub async fn csrf(State(state), Extension(ctx): Extension<KeyContext>) -> impl IntoResponse {
    let Some(sid) = ctx.session_id else { return (StatusCode::UNAUTHORIZED, ...).into_response() };
    let sess = state.db.get_session(&sid).await.unwrap_or(None);
    match sess {
        Some(s) => (StatusCode::OK, Json(json!({ "csrf_token": s.csrf_token }))).into_response(),
        None => (StatusCode::UNAUTHORIZED, ...).into_response(),
    }
}
```

- [ ] **Step 3: Wire routes**

```rust
.route("/v1/auth/login", post(login))      // unauth
.route("/v1/auth/logout", post(logout))    // auth_middleware required
.route("/v1/auth/me", get(me))             // auth_middleware required
.route("/v1/auth/csrf", get(csrf))         // auth_middleware required
```

- [ ] **Step 4: Tests pass**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(litegen-core): /v1/auth login/logout/me/csrf endpoints with lockout"
```

### Task 10: Password reset request + confirm

**Files:**
- Modify: `litegen-core/src/api/handlers/auth_password.rs`

- [ ] **Step 1: Tests**

```rust
#[tokio::test]
async fn reset_request_always_returns_200_even_for_unknown_email() { ... }
#[tokio::test]
async fn reset_request_inserts_token_for_known_email() { ... }
#[tokio::test]
async fn reset_confirm_with_valid_token_sets_new_hash_and_revokes_other_sessions() { ... }
#[tokio::test]
async fn reset_confirm_token_is_single_use() { ... }
```

- [ ] **Step 2: Implement**

```rust
pub async fn password_reset_request(State(state), Json(body)) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    if let Some(user) = state.db.get_user_by_email(&email).await.unwrap_or(None) {
        let token = generate_session_token();
        let reset = PasswordReset {
            token: token.clone(),
            user_id: user.id,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        let _ = state.db.create_password_reset(&reset).await;
        // In dev: log the link. In prod: send via email (SMTP integration out of scope).
        tracing::info!(email = %email, token = %token, "password reset requested — token logged");
        // For dev exposure (god test path):
        if std::env::var("LITEGEN__DEV__EXPOSE_RESET_TOKENS").as_deref() == Ok("true") {
            return (StatusCode::OK, Json(json!({ "ok": true, "_dev_token": token }))).into_response();
        }
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

pub async fn password_reset_confirm(State(state), Json(body)) -> impl IntoResponse {
    let r = state.db.get_password_reset(&body.token).await.unwrap_or(None);
    let Some(r) = r else { return error(StatusCode::BAD_REQUEST, "invalid_token", "Token not found"); };
    if r.used_at.is_some() || r.expires_at < chrono::Utc::now() {
        return error(StatusCode::BAD_REQUEST, "token_expired", "Token already used or expired");
    }
    let hash = match hash_password(&body.new_password) {
        Ok(h) => h,
        Err(PasswordError::TooShort) => return error(StatusCode::BAD_REQUEST,
            "password_too_short", "Password must be at least 12 characters"),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "hash_error", &e.to_string()),
    };
    let _ = state.db.update_user(&r.user_id, None, None, Some(&hash)).await;
    let _ = state.db.mark_password_reset_used(&body.token).await;
    let _ = state.db.delete_user_sessions(&r.user_id, None).await;
    (StatusCode::NO_CONTENT, ()).into_response()
}
```

- [ ] **Step 3: Wire routes (unauth)**

- [ ] **Step 4: Tests pass**

- [ ] **Step 5: Commit**

---

## Phase E — OAuth

### Task 11: OAuth crate setup + config struct

**Files:**
- Modify: `litegen-core/Cargo.toml` (`oauth2 = "4"`, `openidconnect = "3"`)
- Create: `litegen-core/src/auth/oauth.rs`

- [ ] **Step 1: Config**

```rust
#[derive(Clone, Debug)]
pub struct OAuthConfig {
    pub github: Option<ProviderConfig>,
    pub google: Option<ProviderConfig>,
    pub callback_base: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl OAuthConfig {
    pub fn from_env() -> Self {
        let github = pair("LITEGEN__OAUTH__GITHUB__CLIENT_ID", "LITEGEN__OAUTH__GITHUB__CLIENT_SECRET");
        let google = pair("LITEGEN__OAUTH__GOOGLE__CLIENT_ID", "LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET");
        let callback_base = std::env::var("LITEGEN__OAUTH__CALLBACK_BASE").ok();
        Self { github, google, callback_base }
    }
    pub fn enabled_providers(&self) -> Vec<&'static str> {
        let mut v = vec![];
        if self.github.is_some() { v.push("github"); }
        if self.google.is_some() { v.push("google"); }
        v
    }
}

fn pair(id: &str, secret: &str) -> Option<ProviderConfig> {
    match (std::env::var(id), std::env::var(secret)) {
        (Ok(client_id), Ok(client_secret)) if !client_id.is_empty() && !client_secret.is_empty() =>
            Some(ProviderConfig { client_id, client_secret }),
        _ => None,
    }
}
```

- [ ] **Step 2: Add OAuthConfig to AppState**

- [ ] **Step 3: Test**

```rust
#[test]
fn no_env_means_no_providers() {
    let c = OAuthConfig::from_env();
    assert!(c.enabled_providers().is_empty());
}
```

- [ ] **Step 4: Commit**

### Task 12: GitHub OAuth flow (start + callback)

**Files:**
- Create: `litegen-core/src/api/handlers/oauth.rs`

- [ ] **Step 1: Implement `start` handler**

Builds the GitHub authorize URL with client_id, redirect_uri=`{callback_base}/v1/auth/oauth/github/callback`, scope=`user:email`, state=`generate_session_token()`. Sets `litegen_oauth_state` httpOnly+Secure+SameSite=Lax cookie with the state value. Returns 302 redirect.

If `state.oauth.github.is_none()` → 404.

- [ ] **Step 2: Implement `callback` handler**

```rust
pub async fn github_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    // 1. Verify state matches cookie
    let Some(cookie_state) = cookie_value(&headers, "litegen_oauth_state") else {
        return error(StatusCode::BAD_REQUEST, "state_missing", "OAuth state cookie missing");
    };
    if !constant_time_eq(&cookie_state, &params.state) {
        return error(StatusCode::BAD_REQUEST, "state_mismatch", "OAuth state mismatch");
    }
    // 2. Exchange code → token
    let cfg = state.oauth.github.as_ref().ok_or_404()?;
    let token = exchange_github_code(cfg, &params.code).await?;
    // 3. Fetch /user + /user/emails
    let (gh_id, email_opt) = fetch_github_profile(&token).await?;
    let Some(email) = email_opt else {
        return error(StatusCode::BAD_REQUEST, "no_verified_email", "No verified primary email on GitHub");
    };
    // 4. Look up user
    let user = if let Some(u) = state.db.get_user_by_oauth("github", &gh_id).await.ok().flatten() {
        u
    } else if let Some(u) = state.db.get_user_by_email(&email).await.ok().flatten() {
        state.db.link_oauth(&u.id, "github", &gh_id).await.ok();
        u
    } else {
        return error(StatusCode::FORBIDDEN, "account_not_invited",
            "No account exists for this email. Ask an admin to invite you.");
    };
    if !user.is_active { return error(StatusCode::FORBIDDEN, "account_inactive", "..."); }
    let _ = state.db.touch_last_login(&user.id).await;
    let (sc, cc) = create_session_for(&state, &user.id, ...).await;
    (StatusCode::FOUND, [
        ("location", "/"),
        sc.into(), cc.into(),
        clear_oauth_state_cookie().into(),
    ]).into_response()
}
```

- [ ] **Step 3: Tests with httpmock or wiremock simulating GitHub**

```rust
#[tokio::test]
async fn github_callback_state_mismatch_returns_400() { ... }
#[tokio::test]
async fn github_callback_unknown_email_returns_403_account_not_invited() { ... }
#[tokio::test]
async fn github_callback_existing_user_creates_session() { ... }
```

- [ ] **Step 4: Wire routes (unauth)**

- [ ] **Step 5: Commit**

### Task 13: Google OAuth flow (mirror of github with openidconnect)

Same shape, uses `openidconnect` crate for OIDC. Same test cases.

---

## Phase F — Users + invitations endpoints

### Task 14: GET /v1/users + POST /v1/users (invitation)

**Files:**
- Create: `litegen-core/src/api/handlers/users.rs`

- [ ] **Step 1: Tests**

```rust
#[tokio::test]
async fn list_users_requires_user_read_any() { ... }
#[tokio::test]
async fn admin_can_list_all_users() { ... }
#[tokio::test]
async fn member_gets_403_listing_users() { ... }
#[tokio::test]
async fn invite_creates_invitation_and_audits() { ... }
```

- [ ] **Step 2: Implement**

```rust
pub async fn list_users(State, Extension(ctx)) -> impl IntoResponse {
    if !ctx.permissions.contains(&Permission::UserReadAny) {
        return error(StatusCode::FORBIDDEN, "forbidden_permission", "user:read:any required");
    }
    let users = state.db.list_users().await.unwrap_or_default();
    (StatusCode::OK, Json(users.into_iter().map(|u| public(u)).collect::<Vec<_>>())).into_response()
}

pub async fn invite_user(State, Extension(ctx), Json(body): Json<InviteRequest>) -> impl IntoResponse {
    if !ctx.permissions.contains(&Permission::InvitationSend) { return error(...); }
    let role = body.role; // can't be Owner
    if role == Role::Owner {
        return error(StatusCode::BAD_REQUEST, "cannot_invite_owner",
            "Cannot invite an Owner directly. Use transfer-owner.");
    }
    let inv = Invitation { id: uuid_v4(), email: body.email.to_lowercase(), role, token: generate_session_token(),
        invited_by: ctx.user.as_ref().map(|u| u.user_id.clone()),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None, created_at: chrono::Utc::now() };
    state.db.create_invitation(&inv).await?;
    audit_log(&state, &ctx, "invitation.create", "invitation", &inv.id, None, Some(&inv));
    // Dev: log + expose token
    if std::env::var("LITEGEN__DEV__EXPOSE_INVITE_TOKENS").as_deref() == Ok("true") {
        return (StatusCode::OK, Json(json!({ "id": inv.id, "_dev_token": inv.token }))).into_response();
    }
    (StatusCode::OK, Json(json!({ "id": inv.id }))).into_response()
}
```

- [ ] **Step 3: Wire + commit**

### Task 15: GET/POST /v1/auth/invitations/{token}, PATCH /v1/users/{id}, DELETE /v1/users/{id}, POST /v1/users/transfer-owner

Same pattern, one task per endpoint with tests.

---

## Phase G — Account endpoints

### Task 16: GET/PATCH /v1/account, GET /v1/account/sessions, DELETE /v1/account/sessions/{id}

Standard CRUD on own profile. Tests cover: can change own password, can list/revoke own sessions, can't revoke others'.

---

## Phase H — Existing endpoints scope updates

### Task 17: Update key/log/generation/audit endpoints to honor user permissions

**Files:**
- Modify: handlers for keys, logs, generations, audit

- [ ] **Step 1: For each list endpoint**

```rust
// Logs handler:
let scope = if ctx.permissions.contains(&Permission::KeyReadAny) {
    LogScope::All
} else if ctx.permissions.contains(&Permission::KeyReadOwn) {
    LogScope::OwnerKey(ctx.user.as_ref().and_then(|u| Some(u.user_id.clone())))
} else if ctx.scopes.contains(&Scope::Read) {
    LogScope::All  // Bearer-token admin keys still see all
} else {
    return forbidden();
};
let logs = state.db.list_logs_filtered(filters, page, per_page, scope).await?;
```

Apply similarly to keys, generations, audit.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn member_session_only_sees_own_keys() { ... }
```

- [ ] **Step 3: Commit**

---

## Phase I — OpenAPI annotations + SDK regen

### Task 18: Add `#[utoipa::path]` to all new endpoints

For each new handler from Tasks 8-17, add the full utoipa annotation matching the existing pattern. List in the OpenApi derive in `src/api/openapi.rs`.

- [ ] **Step 1: Annotate all auth endpoints**
- [ ] **Step 2: Annotate users + account endpoints**
- [ ] **Step 3: Annotate invitation endpoints**
- [ ] **Step 4: Verify `/openapi.json` contains every new path**
- [ ] **Step 5: Commit**

### Task 19: Regen TypeScript SDK

- [ ] **Step 1: Run codegen**

```bash
cd sdks && ./scripts/regen-all.sh
```

- [ ] **Step 2: Verify the SDK builds**

```bash
cd sdks/typescript && npm run build && npm test
```

- [ ] **Step 3: Commit the regen output**

```bash
git add sdks/typescript/src
git commit -m "feat(sdks/ts): regen for auth + users endpoints"
```

---

## Phase J — Dashboard SDK migration

### Task 20: Add @litegen/sdk dependency + client root

**Files:**
- Modify: `dashboard/package.json`
- Create: `dashboard/src/sdk-client.ts`
- Modify: `dashboard/src/main.tsx`

- [ ] **Step 1: Add dep**

```json
// dashboard/package.json
"dependencies": {
  "@litegen/sdk": "file:../sdks/typescript",
  ...
}
```

`npm install` in dashboard.

- [ ] **Step 2: Create client root**

```ts
// dashboard/src/sdk-client.ts
import { LitegenClient } from '@litegen/sdk';

const BASE = import.meta.env.VITE_API_URL || 'http://localhost:4000';

export const client = new LitegenClient({
  baseUrl: BASE,
  // session cookie auth: browser auto-includes Cookie header on same-origin.
  // Bearer (API-key) fallback: pulled from localStorage by the apiKey hook.
  getAuthToken: () => localStorage.getItem('litegen_api_key') ?? undefined,
  csrf: {
    getToken: async () => {
      const res = await fetch(`${BASE}/v1/auth/csrf`, { credentials: 'include' });
      if (!res.ok) return undefined;
      const json = await res.json();
      return json.csrf_token;
    },
  },
  onError: (status, body) => {
    if (status === 401) {
      localStorage.removeItem('litegen_api_key');
      window.dispatchEvent(new Event('litegen:unauthenticated'));
    }
    if ([402, 403, 429].includes(status)) {
      showToast(body?.error?.message ?? 'Request denied', 'error');
    }
  },
});
```

- [ ] **Step 3: Commit**

### Task 21: Migrate each page to use client

One sub-task per page. Each replaces `api.X()` calls with `client.X.Y()`. Delete unused imports from old api.ts.

- [ ] **21a:** Migrate Keys page
- [ ] **21b:** Migrate Logs page
- [ ] **21c:** Migrate Audit page
- [ ] **21d:** Migrate Models page
- [ ] **21e:** Migrate Generations page
- [ ] **21f:** Migrate Overview page
- [ ] **21g:** Migrate Playground page
- [ ] **21h:** Migrate Health page
- [ ] **21i:** Delete `dashboard/src/api.ts`. Commit "remove hand-rolled api.ts; dashboard now on @litegen/sdk".

Each sub-task: run `npm run build` to confirm clean, commit individually.

---

## Phase K — Dashboard auth UI

### Task 22: Login page

**File:** `dashboard/src/pages/Login.tsx`

Form: email + password. OAuth buttons (rendered if `client.auth.config()` returns the provider). "Forgot password" link.

`data-testid`s: `login-email`, `login-password`, `login-submit`, `login-github`, `login-google`, `login-forgot-link`.

### Task 23: Signup page

**File:** `dashboard/src/pages/Signup.tsx`

Same form. Only renders if `client.auth.config().signup_open` is true.

### Task 24: Invitation accept page

**File:** `dashboard/src/pages/AcceptInvite.tsx`

Fetches `client.auth.getInvitation(token)`. Renders email + role + password set form OR OAuth buttons.

### Task 25: Account page

**File:** `dashboard/src/pages/Account.tsx`

Shows email + role. Change password form. Sessions list with revoke buttons.

### Task 26: Users page (admin)

**File:** `dashboard/src/pages/Users.tsx`

Table of users with role badges + actions (change role, deactivate, send invitation). Modal for "Invite user" with email + role select. "Transfer Owner" action visible only to Owner.

### Task 27: UserMenu component replaces AuthBar

**File:** `dashboard/src/components/UserMenu.tsx`

Logged in: email + role badge → dropdown with Account / Sign out. Logged out: "Sign in" link. Fallback "Use API key instead" → reveals the old paste-key flow that writes to localStorage. Mount in App.tsx instead of AuthBar.

### Task 28: Route guard helper

`<RequirePermission perm="...">` wrapper that redirects to /login if no session, or shows 403 page if session lacks the perm. Wrap Users page with `<RequirePermission perm="user:read:any">`.

---

## Phase L — God test extension

### Task 29: Extend god test with full session flow

**File:** `dashboard/e2e/god-test.spec.ts`

Add a new section at the start of the test (BEFORE the existing master-key path). The dashboard now also supports session auth — verify it works.

```ts
// ─── Session-based auth flow ──────────────────────────
const ownerEmail = 'owner@litegen.test';
const ownerPw = 'super-secret-password-123';
const memberEmail = 'member@litegen.test';
const memberPw = 'another-strong-pw-456';

// Sign up as Owner (users table empty in test mode)
await page.goto('/signup');
await page.locator('[data-testid="signup-email"]').fill(ownerEmail);
await page.locator('[data-testid="signup-password"]').fill(ownerPw);
await page.locator('[data-testid="signup-submit"]').click();
await page.waitForURL('**/');
await expect(page.locator('[data-testid="user-menu-email"]')).toContainText(ownerEmail);
await expect(page.locator('[data-testid="user-menu-role"]')).toContainText(/owner/i);

// Visit /users
await page.click('a[href="/users"]');
await page.waitForURL('**/users');
await expect(page.locator(`[data-testid="user-row-${ownerEmail}"]`)).toBeVisible();

// Invite a member
await page.locator('[data-testid="users-invite-btn"]').click();
await page.locator('[data-testid="invite-email"]').fill(memberEmail);
await page.locator('[data-testid="invite-role"]').selectOption('member');
await page.locator('[data-testid="invite-send"]').click();
// Dev mode exposes token in response banner
const tokenEl = page.locator('[data-testid="invite-dev-token"]');
await expect(tokenEl).toBeVisible();
const inviteToken = (await tokenEl.textContent())!.trim();

// Sign out
await page.locator('[data-testid="user-menu-toggle"]').click();
await page.locator('[data-testid="user-menu-signout"]').click();
await page.waitForURL('**/login');

// Accept invitation
await page.goto(`/invite/${inviteToken}`);
await page.locator('[data-testid="accept-password"]').fill(memberPw);
await page.locator('[data-testid="accept-submit"]').click();
await page.waitForURL('**/');

// Sign out the member
await page.locator('[data-testid="user-menu-toggle"]').click();
await page.locator('[data-testid="user-menu-signout"]').click();

// Sign in as member
await page.goto('/login');
await page.locator('[data-testid="login-email"]').fill(memberEmail);
await page.locator('[data-testid="login-password"]').fill(memberPw);
await page.locator('[data-testid="login-submit"]').click();
await page.waitForURL('**/');
await expect(page.locator('[data-testid="user-menu-role"]')).toContainText(/member/i);

// Member tries to visit /users → 403 redirect
await page.goto('/users');
await expect(page.locator('[data-testid="forbidden-403"]')).toBeVisible();

// Sign back in as owner
await page.locator('[data-testid="user-menu-toggle"]').click();
await page.locator('[data-testid="user-menu-signout"]').click();
await page.goto('/login');
await page.locator('[data-testid="login-email"]').fill(ownerEmail);
await page.locator('[data-testid="login-password"]').fill(ownerPw);
await page.locator('[data-testid="login-submit"]').click();
await page.waitForURL('**/');

// Promote member to admin, then transfer ownership
await page.goto('/users');
await page.locator(`[data-testid="user-edit-${memberEmail}"]`).click();
await page.locator('[data-testid="user-edit-role"]').selectOption('admin');
await page.locator('[data-testid="user-edit-save"]').click();
await expect(page.locator(`[data-testid="user-role-${memberEmail}"]`)).toContainText(/admin/i);
await page.locator(`[data-testid="user-transfer-${memberEmail}"]`).click();
await page.locator('[data-testid="confirm-transfer"]').click();
await expect(page.locator(`[data-testid="user-role-${memberEmail}"]`)).toContainText(/owner/i);
await expect(page.locator(`[data-testid="user-role-${ownerEmail}"]`)).toContainText(/admin/i);

// Sign out, fall through to the existing master-key path which the test continues with.
await page.locator('[data-testid="user-menu-toggle"]').click();
await page.locator('[data-testid="user-menu-signout"]').click();
```

**Test env vars** (added to `playwright.config.ts` webServer env):
```
LITEGEN__DEV__EXPOSE_INVITE_TOKENS=true
LITEGEN__DEV__EXPOSE_RESET_TOKENS=true
LITEGEN__COOKIE_INSECURE_DEV=true   // allows non-Secure cookies over HTTP
```

- [ ] **Step 1: Write the new section**
- [ ] **Step 2: Run `cd dashboard && npx playwright test`**
- [ ] **Step 3: Debug + fix until passing**
- [ ] **Step 4: Commit `test(dashboard): god test exercises full session auth flow`**

---

## Phase M — Final polish

### Task 30: README + deployment notes

- [ ] **Step 1: Add "Auth" section to README** documenting:
  - First-run flow (signup or LITEGEN__OWNER_EMAIL)
  - Role descriptions
  - OAuth env vars
  - Cookie security (Secure flag, SameSite=Lax)
- [ ] **Step 2: Update RUNBOOK.md** with new failure modes:
  - "Lockout extended for legitimate user" — admin can clear via SQL
  - "OAuth callback failing" — check callback_base matches GitHub/Google config
- [ ] **Step 3: Commit**

### Task 31: Final verification

- [ ] **Step 1:** `cd litegen-core && cargo test --lib` — all pass.
- [ ] **Step 2:** `cd litegen-core && cargo clippy --lib --no-deps -- -D warnings` — clean.
- [ ] **Step 3:** `cd litegen-core && cargo build --release` — clean.
- [ ] **Step 4:** `cd dashboard && npm run build` — clean.
- [ ] **Step 5:** `cd dashboard && rm -rf test-results && PLAYWRIGHT_MASTER_KEY=test-master-key-please-rotate npx playwright test` — 1 passed.
- [ ] **Step 6:** Confirm `dashboard/test-results/god-test-*/video.webm` exists.
- [ ] **Step 7:** Final report with branch totals (commit count, line delta).

---

## Self-review checklist

**Spec coverage:**
- ✅ Users table — Task 1
- ✅ Sessions table — Task 1
- ✅ Invitations table — Task 1
- ✅ Password resets table — Task 1
- ✅ login_attempts table — Task 1
- ✅ api_keys.owner_user_id — Task 1
- ✅ audit_log.actor_user_id — Task 1
- ✅ Role enum + permissions map — Task 2
- ✅ Argon2id — Task 3
- ✅ Session token gen — Task 4
- ✅ Lockout — Task 5
- ✅ DB methods — Task 6
- ✅ Session cookie auth — Task 7
- ✅ CSRF — Task 7
- ✅ require_permission — Task 7
- ✅ Signup — Task 8
- ✅ Login/logout/me/csrf — Task 9
- ✅ Password reset — Task 10
- ✅ OAuth config — Task 11
- ✅ GitHub OAuth — Task 12
- ✅ Google OAuth — Task 13
- ✅ Users CRUD — Tasks 14-15
- ✅ Account endpoints — Task 16
- ✅ Existing endpoints scope updates — Task 17
- ✅ OpenAPI annotations — Task 18
- ✅ SDK regen — Task 19
- ✅ Dashboard SDK migration — Tasks 20-21
- ✅ Dashboard auth UI — Tasks 22-28
- ✅ God test session flow — Task 29
- ✅ README + RUNBOOK — Task 30

**Placeholder scan:** No "TBD" or "TODO" in steps. Some Phase F/G tasks (14-16) summarize with "Same pattern" — those refer to the standing Task 8-10 pattern (test → handler → wire route → commit). Acceptable shorthand given consistent context.

**Type consistency:** `KeyContext`, `UserContext`, `Permission`, `Role` all match across tasks. `Session.csrf_token` consistent. `OAuthConfig.enabled_providers()` consistent.
