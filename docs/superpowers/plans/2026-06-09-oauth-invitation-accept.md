# OAuth Invitation Accept — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an OAuth (Google/GitHub) invitee accept a user-invitation and join the inviter's org, closing the gap left by the invitation-accept password fix (`00399aa`).

**Architecture:** Carry the invite token through the existing OAuth round-trip in a short-lived `litegen_oauth_invite` cookie. The OAuth callback gains an invitation-aware branch that, on a strict verified-email match, creates/links the user, adds them to the invited org with the invited role, and atomically consumes the invite — instead of auto-creating a fresh org or 403ing. No-invite logins are unchanged.

**Tech Stack:** Rust (axum, sqlx, wiremock tests), React/TypeScript dashboard (vitest/tsc).

**Spec:** `docs/superpowers/specs/2026-06-09-oauth-invitation-accept-design.md`

---

## File Structure

- `litegen-core/src/db/trait_def.rs` — `mark_invitation_used` returns `Result<bool>` (won the single-use race?)
- `litegen-core/src/db/sqlite.rs`, `postgres.rs` — atomic conditional `UPDATE … WHERE used_at IS NULL`
- `litegen-core/src/db/sqlite_tests.rs` — atomic single-use test
- `litegen-core/src/api/handlers/users.rs` — `accept_invitation` gates on the atomic consume result
- `litegen-core/src/api/handlers/oauth.rs` — invite cookie helpers, `StartParams.invite`, `apply_invitation_oauth`, callback branch, redirect-on-error, tests
- `dashboard/src/pages/AcceptInvite.tsx` — OAuth buttons, conditional password form, `invite_error` banner

---

## Task 1: Atomic `mark_invitation_used` (DB layer)

**Files:**
- Modify: `litegen-core/src/db/trait_def.rs:350`
- Modify: `litegen-core/src/db/sqlite.rs:1129-1135`
- Modify: `litegen-core/src/db/postgres.rs:1148-1154`
- Test: `litegen-core/src/db/sqlite_tests.rs`

- [ ] **Step 1: Write the failing test** (append to `sqlite_tests.rs`; it uses `in_memory_db()`, `Uuid`, `chrono::Utc` like the other tests — add `use crate::api::middleware::DEFAULT_ORG_ID;` and `use crate::types::{Invitation, Role};` to the test module's imports if not already present)

```rust
#[tokio::test]
async fn mark_invitation_used_is_atomic_single_use() {
    let db = in_memory_db().await;
    let inv = Invitation {
        id: format!("inv-{}", Uuid::new_v4()),
        email: "alice@x.com".into(),
        role: Role::Member,
        token: "tok-atomic".into(),
        invited_by: None,
        org_id: DEFAULT_ORG_ID.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };
    db.create_invitation(&inv).await.unwrap();

    assert!(db.mark_invitation_used("tok-atomic").await.unwrap(), "first consume must win");
    assert!(!db.mark_invitation_used("tok-atomic").await.unwrap(), "second consume must lose (already used)");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd litegen-core && cargo test --lib mark_invitation_used_is_atomic_single_use 2>&1 | tail -20`
Expected: COMPILE FAIL — `mark_invitation_used` returns `()` today, so `.unwrap()` on a `bool` and the `assert!(...)` won't typecheck. (That compile failure is the proof the behavior doesn't exist yet.)

- [ ] **Step 3: Change the trait default** (`trait_def.rs:350`)

```rust
    async fn mark_invitation_used(&self, _token: &str) -> Result<bool, sqlx::Error> {
        Ok(true)
    }
```

- [ ] **Step 4: Make the SQLite impl atomic** (`sqlite.rs:1129`)

```rust
    async fn mark_invitation_used(&self, token: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE invitations SET used_at = datetime('now') WHERE token = ? AND used_at IS NULL",
        )
        .bind(token)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
```

- [ ] **Step 5: Make the Postgres impl atomic** (`postgres.rs:1148`)

```rust
    async fn mark_invitation_used(&self, token: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE invitations SET used_at = NOW() WHERE token = $1 AND used_at IS NULL",
        )
        .bind(token)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
```

- [ ] **Step 6: Run the test (now passes) + full lib**

Run: `cargo test --lib mark_invitation_used_is_atomic_single_use 2>&1 | tail -5` → PASS
Run: `cargo test --lib 2>&1 | tail -3` → all pass (the existing `accept_invitation` call `let _ = state.db.mark_invitation_used(&token).await;` still compiles — it discards the whole `Result`).

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/db/trait_def.rs litegen-core/src/db/sqlite.rs litegen-core/src/db/postgres.rs litegen-core/src/db/sqlite_tests.rs
git commit -m "fix(db): make mark_invitation_used atomic single-use (returns bool)"
```

---

## Task 2: Gate password-path `accept_invitation` on the atomic consume

The atomic op from Task 1 only prevents a double-spend if the handler acts on its result. Wire `accept_invitation` to abort when it loses the race. (The existing upfront `used_at` check already rejects *sequential* reuse — its observable behavior is unchanged — so this is concurrent-race hardening; coverage is the Task 1 unit test plus the existing `accept_invitation_*` regressions staying green.)

**Files:**
- Modify: `litegen-core/src/api/handlers/users.rs` (the `let _ = state.db.mark_invitation_used(&token).await;` line, ~388, after membership is added and before the session is created)

- [ ] **Step 1: Replace the fire-and-forget consume with a gated one**

```rust
    // Atomically consume the invitation. If a concurrent request already used it,
    // abort before minting a session (defense-in-depth on top of the used_at check).
    match state.db.mark_invitation_used(&token).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::BAD_REQUEST,
                "invitation_already_used",
                "This invitation has already been used",
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
```

- [ ] **Step 2: Run the existing invitation tests (must stay green)**

Run: `cargo test --lib accept_invitation 2>&1 | tail -8`
Expected: all `accept_invitation_*` tests pass (correct-password, new-user, wrong-password, oauth-only). They reach the consume on the success paths and `mark_invitation_used` returns `true`.

- [ ] **Step 3: Commit**

```bash
git add litegen-core/src/api/handlers/users.rs
git commit -m "fix(invites): abort accept_invitation if the token was already consumed"
```

---

## Task 3: OAuth `start` carries the invite token in a cookie

**Files:**
- Modify: `litegen-core/src/api/handlers/oauth.rs` (`StartParams` ~196, add cookie helpers ~85, `github_start` ~257-264 and `google_start` ~505-512)
- Test: `oauth.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test** (add to `oauth.rs` tests module; `build_google_router` already exists)

```rust
    #[tokio::test]
    async fn oauth_start_with_invite_sets_invite_cookie() {
        let oauth = OAuthConfig {
            google: Some(ProviderConfig { client_id: "g-id".into(), client_secret: "g-secret".into() }),
            callback_base: Some("https://app.example.com".into()),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/start?invite=invtok123&next=/")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(
            cookies.iter().any(|c| c.starts_with("litegen_oauth_invite=invtok123")),
            "start must set the invite cookie; got {:?}", cookies
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib oauth_start_with_invite_sets_invite_cookie 2>&1 | tail -15`
Expected: FAIL — either `StartParams` has no `invite` field (compile) or no `litegen_oauth_invite` cookie is set (assert).

- [ ] **Step 3: Add the `invite` field to `StartParams`** (`oauth.rs:196`)

```rust
#[derive(Debug, Deserialize)]
pub struct StartParams {
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub invite: Option<String>,
}
```

- [ ] **Step 4: Add cookie helpers** (next to `make_oauth_next_cookie`, ~oauth.rs:85)

```rust
/// Build a `Set-Cookie` header value storing the pending invitation token.
fn make_oauth_invite_cookie(token: &str) -> axum::http::HeaderValue {
    let secure = std::env::var("LITEGEN__COOKIE_INSECURE_DEV").as_deref() != Ok("true");
    let secure_str = if secure { "; Secure" } else { "" };
    let val = format!(
        "litegen_oauth_invite={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        urlencoding::encode(token),
        secure_str
    );
    axum::http::HeaderValue::from_str(&val)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static(""))
}

/// Build a `Set-Cookie` header value that clears the `litegen_oauth_invite` cookie.
fn clear_oauth_invite_cookie() -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_static(
        "litegen_oauth_invite=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
    )
}
```

- [ ] **Step 5: Set the cookie in both start handlers**

In `github_start` (after the `if let Some(next) = …` block, ~oauth.rs:264) AND in `google_start` (~oauth.rs:512), add:

```rust
    if let Some(invite) = params.invite.as_deref().filter(|t| !t.is_empty()) {
        resp.headers_mut()
            .append("set-cookie", make_oauth_invite_cookie(invite));
    }
```

- [ ] **Step 6: Run the test (passes)**

Run: `cargo test --lib oauth_start_with_invite_sets_invite_cookie 2>&1 | tail -5` → PASS
(`clear_oauth_invite_cookie` is unused until Task 4 — that's expected; if the unused-fn warning fails the build under `-D warnings`, proceed straight to Task 4 in the same commit, or add `#[allow(dead_code)]` temporarily. Default `cargo test` only warns.)

- [ ] **Step 7: Commit**

```bash
git add litegen-core/src/api/handlers/oauth.rs
git commit -m "feat(oauth): carry invite token through start via litegen_oauth_invite cookie"
```

---

## Task 4: OAuth callback applies the invitation

**Files:**
- Modify: `litegen-core/src/api/handlers/oauth.rs` — add `invite_error_redirect` + `apply_invitation_oauth`; clear invite cookie in `finish_oauth_login`; branch in `handle_google_callback` + `handle_github_callback`
- Test: `oauth.rs` tests

- [ ] **Step 1: Write the first failing test — matching email joins the invited org**

Add this seed helper and test to the `oauth.rs` tests module (`create_user`, `build_google_router`, `build_state_with_oauth` already exist; add `use crate::types::Invitation;` to the test module imports):

```rust
    /// Create an inviter + their org, then invite `invitee_email` into it.
    /// Returns the org id the invitee should join.
    async fn seed_org_and_invite(
        state: &Arc<AppState>,
        invitee_email: &str,
        token: &str,
        role: Role,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> String {
        create_user(state, "inviter@x.com", None, Some("inviter-oauth")).await;
        let inviter = state.db.get_user_by_email("inviter@x.com").await.unwrap().unwrap();
        crate::api::handlers::auth_password::create_org_for_user(
            &state.db, &inviter.id, "inviter@x.com", Some("Acme".to_string()),
        ).await.unwrap();
        let org_id = state.db.list_orgs_for_user(&inviter.id).await.unwrap()[0].0.id.clone();
        let inv = Invitation {
            id: format!("inv-{}", uuid::Uuid::new_v4()),
            email: invitee_email.to_string(),
            role,
            token: token.to_string(),
            invited_by: Some(inviter.id.clone()),
            org_id: org_id.clone(),
            expires_at,
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        state.db.create_invitation(&inv).await.unwrap();
        org_id
    }

    /// Mount Google token + userinfo wiremock returning `email` for `sub`.
    async fn google_mock(server: &MockServer, sub: &str, email: &str, verified: bool) -> OAuthConfig {
        Mock::given(method("POST")).and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "g", "id_token": "id", "expires_in": 3600, "token_type": "Bearer"})))
            .mount(server).await;
        Mock::given(method("GET")).and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": sub, "email": email, "email_verified": verified, "name": "N"})))
            .mount(server).await;
        let uri = server.uri();
        OAuthConfig {
            google: Some(ProviderConfig { client_id: "g-id".into(), client_secret: "g-secret".into() }),
            google_token_base: Some(uri.clone()),
            google_userinfo_base: Some(uri),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn oauth_accept_invite_matching_email_joins_invited_org() {
        let server = MockServer::start().await;
        let oauth = google_mock(&server, "alice-sub", "alice@x.com", true).await;
        let state = build_state_with_oauth(oauth).await; // single_tenant: invite path still works
        let org_id = seed_org_and_invite(
            &state, "alice@x.com", "invtok", Role::Member,
            chrono::Utc::now() + chrono::Duration::days(7),
        ).await;
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(cookies.iter().any(|c| c.contains("litegen_session=")), "session set; got {:?}", cookies);

        let alice = state.db.get_user_by_email("alice@x.com").await.unwrap().expect("alice created");
        assert_eq!(state.db.get_membership(&org_id, &alice.id).await.unwrap(), Some(Role::Member));
        let alice_orgs = state.db.list_orgs_for_user(&alice.id).await.unwrap();
        assert_eq!(alice_orgs.len(), 1, "alice is only in the invited org (no fresh org)");
        assert_eq!(alice_orgs[0].0.id, org_id);
        let inv = state.db.get_invitation("invtok").await.unwrap().unwrap();
        assert!(inv.used_at.is_some(), "invitation consumed");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib oauth_accept_invite_matching_email_joins_invited_org 2>&1 | tail -20`
Expected: FAIL — with no invite branch, single_tenant `resolve_or_create_user` returns `None` → 403 `account_not_invited` (status is FORBIDDEN, not FOUND), and alice is never created.

- [ ] **Step 3: Add `invite_error_redirect` + `apply_invitation_oauth`** (in `oauth.rs`, near the other helpers)

```rust
/// 302 back to the AcceptInvite SPA page with an error code, clearing all OAuth
/// round-trip cookies (incl. the invite cookie).
fn invite_error_redirect(token: &str, code: &str) -> Response {
    let location = format!("/invite/{}?invite_error={}", urlencoding::encode(token), code);
    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(
        "location",
        axum::http::HeaderValue::from_str(&location)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    resp.headers_mut().append("set-cookie", clear_oauth_state_cookie());
    resp.headers_mut().append("set-cookie", clear_oauth_provider_cookie());
    resp.headers_mut().append("set-cookie", clear_oauth_next_cookie());
    resp.headers_mut().append("set-cookie", clear_oauth_invite_cookie());
    resp
}

/// Apply an invitation during an OAuth callback. On success the returned user is
/// created/linked, added to the invited org, and the invite is consumed — caller
/// then mints a session. On any invite failure returns a 302 redirect (Err).
async fn apply_invitation_oauth(
    state: &Arc<AppState>,
    provider: &str,
    oauth_id: &str,
    email: &str,
    invite_token: &str,
) -> Result<String, Response> {
    let inv = match state.db.get_invitation(invite_token).await {
        Ok(Some(i)) => i,
        Ok(None) => return Err(invite_error_redirect(invite_token, "invitation_invalid")),
        Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
    };
    if inv.used_at.is_some() || inv.expires_at < chrono::Utc::now() {
        return Err(invite_error_redirect(invite_token, "invitation_invalid"));
    }
    // Strict verified-email match (both already lowercased by the caller / storage).
    if inv.email.to_lowercase() != email {
        return Err(invite_error_redirect(invite_token, "email_mismatch"));
    }

    // Resolve the user without auto-creating a fresh org.
    let user = match state.db.get_user_by_oauth(provider, oauth_id).await {
        Ok(Some(u)) => u,
        Ok(None) => match state.db.get_user_by_email(email).await {
            Ok(Some(u)) => {
                let _ = state.db.link_oauth(&u.id, provider, oauth_id).await;
                u
            }
            Ok(None) => {
                let now = chrono::Utc::now();
                let u = User {
                    id: format!("user-{}", uuid::Uuid::new_v4()),
                    email: email.to_string(),
                    password_hash: None,
                    role: inv.role,
                    oauth_github_id: if provider == "github" { Some(oauth_id.to_string()) } else { None },
                    oauth_google_id: if provider == "google" { Some(oauth_id.to_string()) } else { None },
                    created_at: now,
                    updated_at: now,
                    last_login_at: None,
                    is_active: true,
                };
                if let Err(e) = state.db.create_user(&u).await {
                    return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()));
                }
                u
            }
            Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
        },
        Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
    };

    if !user.is_active {
        return Err(invite_error_redirect(invite_token, "account_inactive"));
    }

    // Add membership (idempotent).
    if state.db.get_membership(&inv.org_id, &user.id).await.ok().flatten().is_none() {
        if let Err(e) = state.db.add_org_member(&inv.org_id, &user.id, inv.role).await {
            return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()));
        }
    }

    // Atomically consume — if we lost a race, treat as already used.
    match state.db.mark_invitation_used(invite_token).await {
        Ok(true) => {}
        Ok(false) => return Err(invite_error_redirect(invite_token, "invitation_invalid")),
        Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
    }

    Ok(user.id)
}
```

- [ ] **Step 4: Clear the invite cookie on success** (`finish_oauth_login`, after the other `clear_*` appends, ~oauth.rs:187)

```rust
            resp.headers_mut().append("set-cookie", clear_oauth_invite_cookie());
```

- [ ] **Step 5: Branch in both callbacks**

In `handle_google_callback`, immediately before `let user = match resolve_or_create_user(state, "google", &google_id, &email).await {` (~oauth.rs:664), insert:

```rust
    // Invitation-aware path: an invite cookie means "join the inviter's org",
    // not the normal resolve/auto-create flow.
    if let Some(invite_token) = cookie_value(headers, "litegen_oauth_invite") {
        return match apply_invitation_oauth(state, "google", &google_id, &email, &invite_token).await {
            Ok(user_id) => finish_oauth_login(state, &user_id, headers).await,
            Err(resp) => resp,
        };
    }
```

In `handle_github_callback`, immediately before `let user = match resolve_or_create_user(state, "github", &gh_id, &email).await {` (~oauth.rs:433), insert the same block with `"github"` and `&gh_id`.

- [ ] **Step 6: Run the matching-email test (passes) + full suite**

Run: `cargo test --lib oauth_accept_invite_matching_email_joins_invited_org 2>&1 | tail -5` → PASS
Run: `cargo test --lib 2>&1 | tail -3` → all pass (existing OAuth tests have no invite cookie → unchanged path).

- [ ] **Step 7: Add the rejection + existing-user tests**

```rust
    #[tokio::test]
    async fn oauth_accept_invite_email_mismatch_is_rejected() {
        let server = MockServer::start().await;
        // Invitee signs in as eve@x.com but the invite is for alice@x.com.
        let oauth = google_mock(&server, "eve-sub", "eve@x.com", true).await;
        let state = build_state_with_oauth(oauth).await;
        seed_org_and_invite(&state, "alice@x.com", "invtok", Role::Member,
            chrono::Utc::now() + chrono::Duration::days(7)).await;
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.contains("/invite/invtok") && loc.contains("invite_error=email_mismatch"), "got {}", loc);
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(!cookies.iter().any(|c| c.contains("litegen_session=")), "no session on mismatch");
        assert!(state.db.get_user_by_email("eve@x.com").await.unwrap().is_none(), "eve not created");
        let inv = state.db.get_invitation("invtok").await.unwrap().unwrap();
        assert!(inv.used_at.is_none(), "invite not consumed on mismatch");
    }

    #[tokio::test]
    async fn oauth_accept_invite_expired_is_rejected() {
        let server = MockServer::start().await;
        let oauth = google_mock(&server, "alice-sub", "alice@x.com", true).await;
        let state = build_state_with_oauth(oauth).await;
        seed_org_and_invite(&state, "alice@x.com", "invtok", Role::Member,
            chrono::Utc::now() - chrono::Duration::minutes(1)).await; // already expired
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.contains("invite_error=invitation_invalid"), "got {}", loc);
        assert!(state.db.get_invitation("invtok").await.unwrap().unwrap().used_at.is_none());
    }

    #[tokio::test]
    async fn oauth_accept_invite_existing_user_joins_no_duplicate() {
        let server = MockServer::start().await;
        let oauth = google_mock(&server, "alice-sub", "alice@x.com", true).await;
        let state = build_state_with_oauth(oauth).await;
        let org_id = seed_org_and_invite(&state, "alice@x.com", "invtok", Role::Admin,
            chrono::Utc::now() + chrono::Duration::days(7)).await;
        // Alice already exists with this google id.
        create_user(&state, "alice@x.com", None, Some("alice-sub")).await;
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let alice = state.db.get_user_by_oauth("google", "alice-sub").await.unwrap().unwrap();
        assert_eq!(state.db.get_membership(&org_id, &alice.id).await.unwrap(), Some(Role::Admin));
        // exactly one user with this email (no duplicate created)
        assert_eq!(alice.email, "alice@x.com");
    }
```

- [ ] **Step 8: Run all new tests + full suite**

Run: `cargo test --lib oauth_accept_invite 2>&1 | tail -8` → 4 pass
Run: `cargo test --lib 2>&1 | tail -3` → all pass

- [ ] **Step 9: Commit**

```bash
git add litegen-core/src/api/handlers/oauth.rs
git commit -m "feat(oauth): apply invitation in callback (strict email match, join invited org)"
```

---

## Task 5: Dashboard `AcceptInvite` — OAuth buttons + conditional password + error banner

**Files:**
- Modify: `dashboard/src/pages/AcceptInvite.tsx`

- [ ] **Step 1: Add imports + helpers** (top of `AcceptInvite.tsx`)

Change the import line to pull in `API_BASE` and the auth-config type:

```tsx
import { client, API_BASE } from '../sdk-client';
import { LiteGenAPIError, type AuthConfigResponse } from '@litegen/sdk';
```

Add above the component:

```tsx
function oauthAccept(provider: 'github' | 'google', token: string) {
  window.location.href =
    `${API_BASE}/v1/auth/oauth/${provider}/start?invite=${encodeURIComponent(token)}&next=${encodeURIComponent('/')}`;
}

const INVITE_ERROR_MESSAGES: Record<string, string> = {
  email_mismatch: 'That account’s email does not match this invitation. Sign in with the invited email.',
  invitation_invalid: 'This invitation is no longer valid (already used or expired).',
  account_inactive: 'This account is inactive. Contact an administrator.',
};
```

- [ ] **Step 2: Add auth-config + invite-error state inside the component**

```tsx
  const [authConfig, setAuthConfig] = useState<AuthConfigResponse | null>(null);
  const inviteErrorCode = new URLSearchParams(window.location.search).get('invite_error') ?? '';

  useEffect(() => {
    client.auth.config()
      .then(setAuthConfig)
      .catch(() => setAuthConfig({ password_enabled: true, providers_enabled: ['github', 'google'], signup_open: false }));
  }, []);

  const providers = authConfig?.providers_enabled ?? ['github', 'google'];
  const passwordEnabled = authConfig?.password_enabled ?? true;
```

- [ ] **Step 3: Render the invite-error banner + OAuth buttons, gate the password form**

In the returned JSX, immediately under the invitation email/role box, add:

```tsx
        {inviteErrorCode && (
          <div data-testid="invite-error" style={{ marginBottom: 16, padding: '10px 14px', background: '#3d1a1a', border: '1px solid #f85149', borderRadius: 6, color: '#f85149', fontSize: 13 }}>
            {INVITE_ERROR_MESSAGES[inviteErrorCode] ?? 'Could not accept this invitation.'}
          </div>
        )}

        {(providers.includes('google') || providers.includes('github')) && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginBottom: passwordEnabled ? 20 : 0 }}>
            {providers.includes('google') && (
              <button type="button" data-testid="accept-oauth-google" className="btn" onClick={() => oauthAccept('google', token!)}>
                Continue with Google
              </button>
            )}
            {providers.includes('github') && (
              <button type="button" data-testid="accept-oauth-github" className="btn" onClick={() => oauthAccept('github', token!)}>
                Continue with GitHub
              </button>
            )}
          </div>
        )}
```

Wrap the existing `<form onSubmit={handleSubmit} …>…</form>` in:

```tsx
        {passwordEnabled && (
          <>
            {/* existing password form unchanged */}
          </>
        )}
```

Also change the subtitle text (line ~98) so it isn't misleading when password is disabled:

```tsx
        <p style={{ margin: '0 0 24px', color: '#8b949e', fontSize: 13, textAlign: 'center' }}>
          {passwordEnabled ? 'Continue with a provider, or set a password' : 'Continue with your provider to join'}
        </p>
```

- [ ] **Step 4: Rebuild the SDK (for `API_BASE`/types) and typecheck the dashboard**

Run:
```bash
cd sdks/typescript && npm run build && cd ../../dashboard && npx tsc --noEmit && echo "tsc OK"
```
Expected: SDK builds; dashboard `tsc` exits 0. (`API_BASE` is already exported from `sdk-client.ts`; no SDK change is required for this task.)

- [ ] **Step 5: Manual smoke (optional but recommended)**

Run the dashboard dev server, open `/invite/<token>?invite_error=email_mismatch`, confirm the banner renders and "Continue with Google" navigates to `…/v1/auth/oauth/google/start?invite=<token>&next=%2F`.

- [ ] **Step 6: Commit**

```bash
git add dashboard/src/pages/AcceptInvite.tsx
git commit -m "feat(dashboard): OAuth accept on AcceptInvite (buttons + conditional password + error banner)"
```

---

## Final verification

- [ ] `cd litegen-core && cargo test --lib 2>&1 | tail -3` — all backend tests pass
- [ ] `cd dashboard && npx tsc --noEmit` — clean
- [ ] Review the full diff for scope: only the files in the File Structure section changed.

## Deploy (after approval)

- Backend (Tasks 1–4): `node deploy.js proxy`
- Dashboard (Task 5): `node deploy.js web`
