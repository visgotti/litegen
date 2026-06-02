---
title: Users + roles + permissions, password+OAuth auth, dashboard on @litegen/sdk
status: approved
owner: joe
date: 2026-05-29
---

# Goal

Add proper user accounts to litegen on top of the existing API-key model. Users authenticate via email+password or OAuth (GitHub + Google), get a server-side session cookie, and act through a role-based permission system (Owner, Admin, Member, Viewer). API keys keep working unchanged for programmatic clients; they now also have an owner-user. Dashboard migrates to the auto-generated `@litegen/sdk` so the SDK is dogfood-tested by the god Playwright test.

# Non-goals

- Multi-tenant orgs/teams. Single shared tenant; roles are global.
- Custom roles or per-permission UI editing. Roles are predefined in code.
- SSO (SAML, Okta). OAuth only via GitHub + Google.
- Magic-link auth.
- 2FA/MFA. (Worth a follow-up but not in this scope.)
- Audit-log retention policy.

# Data model

## New tables

### `users`

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PRIMARY KEY | UUID v4 |
| `email` | TEXT NOT NULL UNIQUE | lowercased, validated |
| `password_hash` | TEXT NULL | Argon2id PHC string. NULL for OAuth-only accounts that haven't set a password. |
| `role` | TEXT NOT NULL | `owner` \| `admin` \| `member` \| `viewer` |
| `oauth_github_id` | TEXT NULL UNIQUE | GitHub numeric id as string |
| `oauth_google_id` | TEXT NULL UNIQUE | Google `sub` |
| `created_at` | TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP | |
| `updated_at` | TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP | |
| `last_login_at` | TIMESTAMP NULL | updated on each successful login |
| `is_active` | INTEGER NOT NULL DEFAULT 1 | soft-deletes via this flag |

### `sessions`

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PRIMARY KEY | 32-byte random hex (64 chars) |
| `user_id` | TEXT NOT NULL REFERENCES users(id) | |
| `created_at` | TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP | |
| `expires_at` | TIMESTAMP NOT NULL | |
| `ip` | TEXT NULL | best-effort |
| `user_agent` | TEXT NULL | truncated to 256 chars |
| `csrf_token` | TEXT NOT NULL | 32-byte random hex, rotated daily |

Index on `(user_id, expires_at)` for `GET /v1/account/sessions`.

### `invitations`

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PRIMARY KEY | UUID |
| `email` | TEXT NOT NULL | lowercased |
| `role` | TEXT NOT NULL | role to assign on accept |
| `token` | TEXT NOT NULL UNIQUE | 32-byte random hex |
| `invited_by` | TEXT NULL REFERENCES users(id) | NULL when sent by master-key |
| `expires_at` | TIMESTAMP NOT NULL | 7 days from creation |
| `used_at` | TIMESTAMP NULL | NULL until accepted |
| `created_at` | TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP | |

### `password_resets`

| Column | Type | Notes |
|---|---|---|
| `token` | TEXT PRIMARY KEY | 32-byte random hex |
| `user_id` | TEXT NOT NULL REFERENCES users(id) | |
| `expires_at` | TIMESTAMP NOT NULL | 1 hour from creation |
| `used_at` | TIMESTAMP NULL | |
| `created_at` | TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP | |

### `login_attempts`

| Column | Type | Notes |
|---|---|---|
| `email` | TEXT NOT NULL | identifier for lockout |
| `attempted_at` | TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP | |
| `success` | INTEGER NOT NULL | 0/1 |

Index on `(email, attempted_at DESC)`. Rows older than 1 hour are pruned at write time.

## Altered tables

- `api_keys`: add `owner_user_id TEXT NULL REFERENCES users(id)`. Existing rows: NULL (treated as system keys, manageable by Owner + Admin only). Future inserts: filled from session user.
- `audit_log`: add `actor_user_id TEXT NULL REFERENCES users(id)`. Existing `actor_key_id` + `actor_label` keep working. When the actor is a session user, `actor_label` = email.

Migration: numbered after the existing series. Sqlite + Postgres mirrors.

# Roles → permissions

Permissions encoded as `subject:verb[:scope]` strings, all known to `auth/permissions.rs`:

```
user:read:self        user:read:any        user:write:any        user:delete:any
key:read:own          key:read:any         key:write:own         key:write:any
key:delete:own        key:delete:any       key:test_webhook:own  key:test_webhook:any
generation:create     generation:read:own  generation:read:any   generation:cancel:own
                                                                  generation:cancel:any
audit:read            cache:clear          system:config         system:transfer_owner
invitation:send       invitation:revoke    session:revoke:own    session:revoke:any
```

Role → permission set:

- **Owner**: all of the above.
- **Admin**: everything except `system:transfer_owner` and `user:delete:any` targeting Owner role (enforced at call site).
- **Member**: `user:read:self`, `key:*:own`, `key:test_webhook:own`, `generation:create`, `generation:read:own`, `generation:cancel:own`, `session:revoke:own`.
- **Viewer**: `user:read:self`, `key:read:own`, `generation:create`, `generation:read:own`, `session:revoke:own`.

`Role::permissions() -> &'static [Permission]` returns the set. The `require_permission!` macro / helper rejects with 403 `forbidden_permission` listing the missing permission.

The existing scope-based auth on API keys (`generate`, `read`, `admin`) keeps working unchanged — those are independent of role-based perms. Bearer-token requests use scopes; cookie-session requests use role permissions.

# Auth flows

## Password

- `POST /v1/auth/signup` — body `{email, password}`.
  - Succeeds only if `users` table empty (or `LITEGEN__OWNER_EMAIL` set and matches).
  - Creates Owner with Argon2id-hashed password.
  - Returns the session cookie immediately.
- `POST /v1/auth/login` — body `{email, password}`. Returns session cookie. Constant-time email lookup (always run Argon2 verify against a dummy hash if user not found).
- `POST /v1/auth/logout` — clears cookie + deletes session row.
- `POST /v1/auth/password-reset/request` — body `{email}`. Always returns 200 (no enumeration leak). If user exists, generate token, store in `password_resets`, log/email the link.
- `POST /v1/auth/password-reset/confirm` — body `{token, new_password}`. Verifies + sets new hash + invalidates token + revokes all other sessions for that user.

### Password rules
- Minimum 12 chars (enforced server-side).
- No max length cap up to 256 chars (Argon2 handles it).
- Hash with Argon2id, params: memory=64 MiB, iterations=3, parallelism=1 (OWASP 2024 recommendation).
- `password_hash::verify` always runs even when user not found, with a precomputed dummy hash, to keep timing constant.

### Lockout
- Track failed attempts in `login_attempts`.
- 5 failures within 15 minutes → 15-min lockout for that email.
- Lockout returns `429 Too Many Requests` with `Retry-After: <seconds>`.

## OAuth

Per provider (github, google):

- `GET /v1/auth/oauth/{provider}/start?next=/path` — generate random `state` (32 bytes), set as signed httpOnly cookie `litegen_oauth_state`, redirect to provider's authorize URL.
- `GET /v1/auth/oauth/{provider}/callback?code=&state=` — verify state cookie matches, exchange code → tokens → fetch profile (`/user` or `/userinfo`).
  - Look up user by `oauth_<provider>_id` first.
  - If not found, look up by email. If found and `oauth_<provider>_id` is NULL, link the IDs (only after verifying email-verified flag from provider).
  - If not found by either, **fail with 403 `account_not_invited`** — OAuth is only for already-invited or pre-existing users.
  - On success, create session and redirect to `next` or `/`.

OAuth secrets via env vars only (never yaml):
```
LITEGEN__OAUTH__GITHUB__CLIENT_ID=...
LITEGEN__OAUTH__GITHUB__CLIENT_SECRET=...
LITEGEN__OAUTH__GOOGLE__CLIENT_ID=...
LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET=...
LITEGEN__OAUTH__CALLBACK_BASE=https://litegen.example.com
```

If either provider's CLIENT_ID/SECRET are unset, those routes return 404 and the UI hides the corresponding button.

Use the `openidconnect` crate for Google (full OIDC) and `oauth2` crate for GitHub (which doesn't speak OIDC). Both crates are well-maintained and audited.

## Session cookie

- Name: `litegen_session`.
- Value: 32-byte random hex (from `rand::rngs::OsRng`).
- Attributes: `HttpOnly; Secure; SameSite=Lax; Path=/`.
- TTL: 7 days, sliding window — every request bumps `expires_at` by another 7 days if the session is within 24 hours of expiry.
- Stored server-side in `sessions` table. DB-backed = revocable by row delete.

### `Secure` attribute behavior
- Always set in production. In dev (when `LITEGEN__SERVER__BIND_HOST=127.0.0.1` and `LITEGEN__COOKIE_INSECURE_DEV=true`), drop the `Secure` flag so cookies work over plain HTTP.

## CSRF

- `SameSite=Lax` handles cross-site GETs from forms.
- For mutating requests (POST/PUT/PATCH/DELETE) authenticated via cookie session, additionally require `X-CSRF-Token` header matching the session's `csrf_token`.
- GET `/v1/auth/csrf` returns the current token to the dashboard.
- Bearer-token requests skip CSRF (they're already same-origin or programmatic).

# Auth middleware extension

Existing middleware in `src/api/middleware/auth.rs` extends:

1. If `Authorization: Bearer <token>` is present → existing API-key path (Bearer wins).
2. Else if `Cookie: litegen_session=<id>` is present:
   - Look up session in DB.
   - If expired → delete row, 401.
   - If valid → load user → build `KeyContext { key_id: None, user: Some(UserContext { id, email, role }), scopes: [], permissions: role.permissions() }`.
   - For mutating verbs, enforce CSRF token.
3. Else → 401.

`KeyContext` grows `user: Option<UserContext>` and `permissions: Vec<Permission>`. Existing scope checks unchanged for Bearer auth.

New `require_permission(perm: Permission)` middleware factory enforces user-perm checks on routes that need them. Existing `require_scope(...)` continues to apply to Bearer-authenticated endpoints.

A route can require EITHER a scope (for Bearer) OR a permission (for session). The router decides per route:

```rust
.route("/v1/keys", get(list_keys)
    .layer(require_either(Scope::Read, Permission::KeyReadOwn)))
```

`require_either` checks the request's auth mode and dispatches to the appropriate check.

# API endpoints

New (all under existing scope guards / require_permission):

- `POST /v1/auth/signup` (open during bootstrap)
- `POST /v1/auth/login`
- `POST /v1/auth/logout`
- `GET /v1/auth/me` — returns `{user: {id, email, role}}` or 401 — used by dashboard to render
- `GET /v1/auth/csrf` — returns current CSRF token (cookie session only)
- `POST /v1/auth/password-reset/request`
- `POST /v1/auth/password-reset/confirm`
- `GET /v1/auth/oauth/{provider}/start`
- `GET /v1/auth/oauth/{provider}/callback`
- `GET /v1/users` — `user:read:any`
- `POST /v1/users` — `invitation:send` — creates invitation
- `PATCH /v1/users/{id}` — `user:write:any` — change role, deactivate
- `DELETE /v1/users/{id}` — `user:delete:any` — soft-delete (set `is_active=0`)
- `POST /v1/users/transfer-owner` — `system:transfer_owner` — body `{new_owner_id}`
- `GET /v1/auth/invitations/{token}` — public, returns `{email, role, expires_at}` or 404
- `POST /v1/auth/invitations/{token}/accept` — body `{password?}`; password set if provided, else OAuth flow follows
- `GET /v1/account` — own profile
- `PATCH /v1/account` — change own password / email
- `GET /v1/account/sessions` — `session:revoke:own`
- `DELETE /v1/account/sessions/{id}` — `session:revoke:own`

Existing endpoints get scoped:
- `GET /v1/keys` — `key:read:own` (member sees own) or `key:read:any` (admin sees all). Returns filtered set based on permission.
- `POST /v1/keys` — sets `owner_user_id` from session user.
- All other key endpoints already require admin scope; for session auth, require `key:write:any` (admin/owner) or `key:write:own` if `owner_user_id == session user`.
- `GET /v1/logs`, `GET /v1/generations` — filtered to own when permission is `*:own`.
- `GET /v1/audit` — `audit:read`.

# Bootstrap

On startup:
1. Run migrations.
2. Query `SELECT COUNT(*) FROM users`.
3. If 0:
   - If `LITEGEN__OWNER_EMAIL` is set → log "Awaiting first signup; only email <X> can claim owner".
   - Else → log a warning every minute "No users exist; any signup will become Owner. Set LITEGEN__OWNER_EMAIL to lock down."
4. If >0: signup endpoint returns 409 `signup_closed`.

# Frontend (dashboard)

## SDK migration (separate workstream within this spec)

Replace hand-rolled `dashboard/src/api.ts` with `@litegen/sdk`:

1. The SDK is auto-generated from `/openapi.json`. Make sure every endpoint added in this spec has full `#[utoipa::path]` annotations BEFORE running codegen.
2. `dashboard/package.json` adds `"@litegen/sdk": "file:../sdks/typescript"` as a workspace dep.
3. Each page swaps `api.getKeys()` → `client.keys.list()` etc. The SDK organizes by resource (`client.keys`, `client.models`, `client.logs`, `client.generations`, `client.audit`, `client.auth`, `client.users`, `client.account`).
4. Construct the client once at app root with auth mode: either Bearer-from-localStorage or session-cookie (auto-attached by browser).
5. Existing `apiFetch` helper retires. Toast triggers (401 clears state, 402/403/429 surface error) move into a single SDK middleware (the SDK supports a `fetchOverride` for this).
6. Hand-rolled JSON validation in dashboard goes away — SDK types are the contract.

## New pages

- `/login` — form + provider buttons. Form fields: email, password, submit. Buttons rendered only if the corresponding env vars are set (SDK exposes `client.auth.config()` returning `{providers_enabled: ["github", "google"], signup_open: true}`).
- `/signup` — only renders if `client.auth.config().signup_open === true`. Same email/password form.
- `/invite/{token}` — fetches `client.auth.getInvitation(token)`, shows email + role, then password set form or OAuth buttons.
- `/account` — current profile, change password form, sessions list with revoke buttons, link to own keys page.
- `/users` (admin/owner only) — table of users with role badge, last login, actions (change role, deactivate, send invitation). Modal for "Invite user" with email + role select.

## Replace AuthBar with UserMenu

- Logged in: shows email + role badge → dropdown with Account / Sign out.
- Logged out: shows "Sign in" link.
- Fallback link "Use API key instead" → reveals the existing paste-key flow that writes to localStorage (preserved for power users + the existing Playwright test path).

## Permission-guided rendering

The dashboard checks `client.auth.me().role` and hides UI a user can't act on. Keys actions disabled (with tooltip "requires admin permission") rather than removed, so user knows the feature exists.

# Backward compatibility

- Existing Playwright god test uses master-key (Bearer) auth → keeps passing as-is.
- Existing API keys with `owner_user_id=NULL` are "system keys" — Owner + Admin can manage them, Member/Viewer cannot.
- The hand-rolled `dashboard/src/api.ts` is removed atomically with the SDK migration; no dual-path period.

# Security checklist

| Concern | Mitigation |
|---|---|
| Password storage | Argon2id, OWASP 2024 params |
| Login timing leak | Always run Argon2 verify (dummy hash on user-not-found) |
| Brute force | 5 fail/15min lockout per email |
| Session hijacking | HttpOnly + Secure + SameSite=Lax cookie, DB-backed revocation |
| CSRF | SameSite=Lax + X-CSRF-Token for mutating session requests |
| OAuth CSRF | Random `state` token in signed httpOnly cookie |
| User enumeration via password reset | Always 200 regardless of email existence |
| OAuth account squatting | Auto-link only when email verified by provider |
| Random OAuth signup | OAuth callback rejects unknown emails |
| Open signup attack | `LITEGEN__OWNER_EMAIL` env-var gate (recommended) |
| Privilege escalation via direct DB write | All role checks server-side, no client-trusted role field |
| Password reset token reuse | Single-use, 1hr TTL |
| Invitation token reuse | Single-use, 7d TTL |
| Insecure HTTP in prod | `Secure` cookie attribute enforced unless explicit dev override |
| Constant-time secret comparison | `subtle::ConstantTimeEq` for token compares |

# Testing

## Lib (unit + integration)

- Argon2id round-trip (hash → verify success, hash → verify with wrong password fails, dummy hash verify takes ~equal time).
- Session token entropy (assert 32 bytes, hex-encoded, 64 chars).
- Role → permissions map (all roles, all expected perms present).
- `require_permission` denies missing perm with 403.
- Lockout: 5 fails then 6th returns 429 with `Retry-After`.
- CSRF: mutating request without header returns 403; with valid header succeeds.
- Bootstrap: empty users table → signup creates Owner; signup again returns 409.
- `LITEGEN__OWNER_EMAIL` set → signup with mismatching email returns 403.
- Invitation flow: send → accept with password → user can log in.
- Password reset flow: request → confirm → other sessions revoked.
- OAuth: state-token mismatch returns 400; valid flow creates session.

## E2E (god Playwright test)

Extend the existing single `test(...)` block. The original master-key-Bearer path stays as the first chunk for backward compat. After that, add a full session flow:

1. Sign up via UI as `owner@litegen.test` / strong password (only because the users table starts empty in test mode).
2. Verify `/users` page shows just the owner.
3. Invite `member@litegen.test` as Member role.
4. Sign out.
5. Visit the invitation URL (the test reads the token from the API since no real email is sent — the invitation endpoint can return the token in test mode, gated by a `LITEGEN__DEV__EXPOSE_INVITE_TOKENS=true` env var).
6. Set member password, complete acceptance.
7. Sign in as member.
8. Visit /keys → assert only member's own keys visible (none initially).
9. Create a key as member, verify it appears.
10. Try to visit /users → expect 403 / redirect to /account.
11. Sign out, sign back in as owner.
12. /users page now shows two users; change member role to Admin.
13. Transfer ownership to admin — verify owner role badge moved.
14. Sign out.

The test runs with the `LITEGEN__DEV__EXPOSE_INVITE_TOKENS=true` and `LITEGEN__COOKIE_INSECURE_DEV=true` flags so HTTP works.

The dashboard runs against the real SDK throughout — this validates the SDK against every endpoint touched by the test.

# Implementation order

1. **Schema + traits** — migrations, types, DatabaseStore methods, unit tests for password hashing, sessions, lockout.
2. **Auth middleware extension** — session lookup, CSRF, KeyContext.user field, require_permission helper.
3. **Password auth** — signup/login/logout/me/csrf/password-reset endpoints + tests.
4. **OAuth** — provider clients, start/callback endpoints + state validation + tests.
5. **Users/invitations API** — list, invite, role change, transfer owner + tests.
6. **OpenAPI annotations** — every new endpoint has `#[utoipa::path]` so codegen picks them up.
7. **SDK regen** — run `sdks/scripts/regen-all.sh`, commit. Verify SDK CI passes.
8. **Dashboard SDK migration** — replace api.ts with `@litegen/sdk` calls. Atomic swap.
9. **Dashboard auth UI** — login, signup, invite-accept, account, users pages. UserMenu.
10. **God test extension** — full session flow.
11. **Final polish** — clippy, README "Auth" section, deployment note about `LITEGEN__OWNER_EMAIL` and OAuth secrets.

# Acceptance

- `cargo test --lib` clean.
- `cargo clippy --lib --no-deps -- -D warnings` clean.
- `cargo build --release` clean.
- `cd dashboard && npm run build` clean.
- `cd sdks/typescript && npm run build && npm test` clean.
- `cd dashboard && npx playwright test` → 1 passed, both the master-key-Bearer path AND the full session flow exercised, video recorded at slow-mo.
- Dashboard uses ZERO `fetch()` calls outside the SDK.
