# OAuth Invitation Accept — Design

**Date:** 2026-06-09
**Status:** Draft (pending review)
**Author:** brainstormed with the maintainer

## Context

A code-review security fix (commit `00399aa`) closed an account-takeover hole in
`accept_invitation` (`litegen-core/src/api/handlers/users.rs`): the existing-user
branch now requires the account's **password** before granting a session. That is
correct, but it leaves a gap on an **OAuth-primary deployment** (litegen.ai is
Google-OAuth-first): an invited user who has no password — either a brand-new
invitee or an existing OAuth-only account — can no longer accept an invitation.
The current `AcceptInvite` page is **password-only** ("Set a password to complete
your registration").

This spec adds an **OAuth-based accept path** so invited Google/GitHub users can
join the org they were invited to.

## Goals

- An invited user can accept by signing in with Google (or GitHub) — no password.
- A **new** invitee is created and added to the **inviter's org** with the
  invited role (not a fresh Owner org, and not a 403).
- An **existing** OAuth user (or any existing account) can accept and join.
- The verified OAuth email **must match** the invited email (strict).
- The normal OAuth login flow (no invitation) is **unchanged**.

## Non-goals (out of scope)

- The org-member invite path (`orgs::invite_member`, `/v1/orgs/{id}/members`).
  This spec covers only the **global user-invitation** flow
  (`/v1/auth/invitations/{token}/accept` + the `AcceptInvite` page).
- Reworking the SDK's (currently unusable, fetch-based) OAuth `start`/`callback`
  methods. The dashboard builds OAuth-start URLs via `window.location` already.
- Email-change / multi-email reconciliation. Match is a simple case-insensitive
  equality of the invited email and the verified OAuth email.

## Decisions

1. **Strict email match.** The provider's *verified* email must equal the
   invitation's email (case-insensitive). A mismatch is rejected
   (`invite_email_mismatch`, 403). Rationale: the invitation token is not proof of
   identity; the verified OAuth email is — and only the invited person should join.
2. **Invitation-aware OAuth callback** (vs. a separate "sign in, then POST accept"
   endpoint). For a *new* invitee, the existing callback would auto-create a fresh
   Owner+org (hosted) or 403 (single-tenant) **before** they could accept. Carrying
   the invite token through the OAuth round-trip and applying it *inside* the
   callback handles new + existing invitees in one pass and reuses the existing
   OAuth state/CSRF cookie protection. As a bonus this also enables invite-only
   (`single_tenant`) OAuth onboarding, which is impossible today.

## Design

### Flow

```
AcceptInvite page (/invite/{token})
  └─ "Continue with Google"  →  GET /v1/auth/oauth/google/start?invite={token}&next=/
        start: sets litegen_oauth_{state,provider,next} (existing) + litegen_oauth_invite={token} (new)
        302 → Google
  └─ Google → GET /auth/redirect?code&state  (unified callback, existing)
        callback (existing): verify state, exchange code, fetch verified email
        callback (NEW branch): if litegen_oauth_invite cookie present →
           apply_invitation_oauth(provider, oauth_id, verified_email, invite_token)
        302 → next (dashboard), session + csrf cookies, all oauth_* cookies cleared
```

### Backend

**Start** (`google_start` / `github_start`, `oauth.rs`):
- `StartParams` gains `invite: Option<String>`.
- When `invite` is present, append a `Set-Cookie: litegen_oauth_invite=<token>`
  (HttpOnly, `SameSite=Lax`, `Max-Age=600`, `Secure` unless dev) — same pattern as
  the existing `litegen_oauth_next` cookie. Add a matching `clear_oauth_invite_cookie()`.

**Callback** (`handle_google_callback` / `handle_github_callback`, `oauth.rs`):
- After the verified `email` is resolved (existing code), read
  `litegen_oauth_invite`. If **absent** → existing `resolve_or_create_user` path,
  unchanged. If **present** → new `apply_invitation_oauth(...)` branch:
  1. Load invitation by token. Not found / `used_at` set / expired → **redirect
     302** to the AcceptInvite page (/invite/{token}) with an error param (see below); clear invite +
     state cookies.
  2. `invitation.email.to_lowercase() != email` → **redirect 302** to the accept
     page with `?invite_error=email_mismatch`; clear cookies. **Does not** consume
     the invite.
  3. Resolve the user:
     - existing by `(provider, oauth_id)` → use; else existing by `email` → link
       oauth id, use; else **create** a new `User` (role = `invitation.role`,
       oauth id set, `password_hash = None`). **No fresh org is auto-created.**
  4. Add the user to `invitation.org_id` with `invitation.role` (idempotent — skip
     if already a member, mirroring `accept_invitation`).
  5. **Atomically** mark the invitation used (see below).
  6. `finish_oauth_login(...)` → session + redirect, clearing all oauth_* cookies
     incl. the new invite cookie.
- Shared by both providers via one helper so GitHub + Google behave identically.

**Atomic mark-used (closes the noted TOCTOU):**
- Change `mark_invitation_used` to a conditional update returning whether it won:
  `UPDATE invitations SET used_at = now() WHERE token = ? AND used_at IS NULL`
  returning rows-affected / the row. Both the OAuth path and the existing
  `accept_invitation` password path consume via this, so a race can't double-spend
  one token. (Trait + sqlite + postgres impls.)

**New error codes:** `invite_email_mismatch` (403), reuse
`invitation_already_used` / `invitation_expired` / `invitation_not_found`
(400/404) consistent with `accept_invitation`.

### Dashboard (`AcceptInvite.tsx`)

- Add **"Continue with Google"** (and GitHub, if configured) buttons that
  `window.location.href = ${API_BASE}/v1/auth/oauth/<provider>/start?invite=<token>&next=/`.
- Keep the password form, shown only when password auth is enabled (read the same
  `AuthConfigResponse` the Login/Signup pages use; if password is disabled, show
  OAuth only).
- On an invite error the callback redirects to `/invite/{token}?invite_error=<code>`
  (`email_mismatch` | `invitation_invalid`). The page reads the param and shows a
  friendly message (e.g. "This invitation is for <email> — sign in with that
  account"), rather than the raw-JSON page the existing OAuth error paths return.
  (Success redirects to `next`, default `/`.)

### SDK

- No required change (OAuth start is a browser navigation, not an SDK fetch).
- *Optional, nice-to-have:* `auth.oauthAcceptUrl(provider, token, { next })`
  returning the start URL, so the dashboard doesn't hand-build it. Low priority.

## Security model

- Verified-email-equality is the identity proof; mismatch → 403, no membership,
  no token consumption.
- Reuses the existing OAuth `state` cookie (CSRF for the OAuth round-trip) and
  `Secure`/`HttpOnly`/`SameSite=Lax` cookie attributes.
- Invite cookie is short-lived (10 min) and cleared on every callback outcome.
- Atomic single-use consumption prevents token replay / double-join races.
- A new invitee created via this path is **not** an Owner of a new org — they get
  exactly `invitation.role` in `invitation.org_id`, nothing more.

## Testing (TDD)

Backend (`oauth.rs` tests, existing wiremock harness):
1. `oauth_accept_invite_matching_email_joins_invited_org` — invite alice→orgO
   (member); Google as alice + invite cookie → 302 + session; alice is a `member`
   of orgO; invitation `used_at` set; **no extra org** created for alice.
2. `oauth_accept_invite_email_mismatch_is_rejected` — invite alice; Google as eve
   + invite cookie → 403 `invite_email_mismatch`; eve not added; invite **not** used.
3. `oauth_accept_invite_expired_is_rejected` and `_already_used_is_rejected` → 400.
4. `oauth_accept_invite_existing_user_joins_no_duplicate` — alice already has an
   account; accepts via OAuth → joins orgO, no duplicate user.
5. `oauth_login_without_invite_cookie_is_unchanged` — regression: normal login
   still auto-creates/links per existing behavior.
6. `mark_invitation_used_is_atomic` (db test) — concurrent/repeat consume wins once.

Dashboard: `tsc --noEmit` clean after the page change.

## Open questions / risks

- The dashboard must know which OAuth providers are configured to render the right
  buttons — reuse `AuthConfigResponse` (it already reports enabled providers).
- `next` after accept defaults to `/` (dashboard home); the org-switcher will show
  the newly-joined org. Acceptable.

## Files touched

- `litegen-core/src/api/handlers/oauth.rs` (start param + cookie, callback branch,
  shared `apply_invitation_oauth`, tests)
- `litegen-core/src/db/{trait_def,sqlite,postgres}.rs` (atomic `mark_invitation_used`)
- `dashboard/src/pages/AcceptInvite.tsx` (OAuth buttons, conditional password form)
- (optional) `sdks/typescript/src/client.ts` (`oauthAcceptUrl` helper)
