# LiteGen Operations Runbook

Quick reference for on-call engineers. Each section describes a failure mode, its likely causes, and remediation steps.

---

## High request error rate

**Signals:** `failed_requests` rising in `GET /v1/stats`; alert on `failed_requests / total_requests > threshold`.

1. Check `GET /v1/stats` — compare `failed_requests` trend over recent windows.
2. Check provider health at `GET /health` — look for `healthy: false` entries.
3. Grep structured logs for `"Image generation failed"` / `"Video generation failed"` to find the error message.
4. If failures are provider-scoped, see **Provider returning 5xx repeatedly** below.
5. If failures are broad (all providers), check DB connectivity, process memory, and network egress.

---

## Provider returning 5xx repeatedly

**Signals:** Provider health endpoint shows unhealthy; requests failing with 502.

1. The circuit breaker should auto-trip after consecutive failures — verify by checking logs for `"Circuit breaker open — skipping deployment"` (`"… skipping video deployment"` for video).
2. Once tripped, traffic automatically falls back to the next deployment in the route chain.
3. Check the provider's own status page to determine if it is a transient outage.
4. If you need to force failover immediately, move the failing deployment to the bottom of the route's chain (or remove it) with `PUT /v1/admin/routes` or the dashboard: every replica picks the change up within 15 seconds, about a second with Redis, with no restart. A route changed in the config file (`litegen.yaml`, or the file `--config` or `LITEGEN_CONFIG_FILE_PATH` names) needs a restart: send `SIGTERM`, which drains in-flight requests first. Never send `SIGHUP`: litegen has no reload, and `SIGHUP` stops it at once, dropping in-flight requests ([configuration.md](configuration.md#what-needs-a-restart)).
5. Re-enable when the provider recovers — the circuit breaker resets automatically after the configured open window elapses.

---

## Quota exceeded for a key

**Signals:** Clients receiving 402 or `x-litegen-quota-exceeded: true` header; key holder reports access denied.

1. Fetch the key: `GET /v1/keys/{id}`. Check `tokens_used` vs `token_quota`.
2. **Option A — bump quota:** `PATCH /v1/keys/{id}` with `{"token_quota": <new_limit>}`.
3. **Option B — wait:** There is no automatic monthly reset. Quotas are absolute lifetime usage caps; the admin must manually reset `tokens_used` by patching the DB directly or issuing a new key.
4. If the key should not have a cap, set `token_quota` to `null` via `PATCH`.

---

## A key stopped working after someone left

**Signals:** A key that used to work now gets 401 with "API key is inactive: the person who made it is no longer an active member of its organization". Or an `admin`-scope key now gets 403 on admin routes.

An API key acts for one person, its owner (`owner_user_id`):
- the person who made it, since 2026-11-06. A key made by another key belongs to that key's owner.
- the person who last rotated it, since 2026-11-11. Rotating hands over the only working secret, so `POST /v1/keys/{id}/rotate` from a dashboard session, an OAuth sign-in or a person's key makes that person the owner. A rotation with the master key, or with a key that belongs to the org, keeps the owner the key had.

What that means:
- A key works only while its owner is an active member of the key's org. Removing them from the org, or deactivating their account, ends their keys. Adding them back makes the keys work again.
- A key holds the `admin` scope only while its owner's role in that org could grant it, which means Owner or Admin. If they're demoted to Member, the key keeps `generate` and `read` and loses `admin`. Giving `admin` to such a key with `PATCH /v1/keys/{id}` is refused with 403.
- A key holds the `generate` scope only while its owner's role may start generations, which means Owner, Admin or Member. If they're made a Viewer, the key keeps only `read`: its generations, quotes, status reads, cancels and deletes get 403 `forbidden_scope`, and on `/mcp` it lists no tool that starts or changes a generation. A key that held only `generate` then gets 403 everywhere until they're a Member again.
- A key's spend counts against its owner's member budget, so a rotated key's spend moves to whoever rotated it.

Keys made with the master key belong to the deployment, not to a person, so nobody leaving affects them. A hosted key with no org (none is made today) is refused.

**Keys from before 2026-11-06.** Keys made by keys before then were stored with no owner. Migration 025 (the 2026-11-11 upgrade) gives each one the owner of the key that made it, read from the audit log's `key.create` rows, down any chain of keys. Audit rows are written in the background, so a key whose row was never written stays the org's: it outlives the person behind it until someone revokes or rotates it.

**Rotations from before 2026-11-11.** These can't be traced: the audit log recorded dashboard actions as `master-key`, and a rotation didn't change the owner. If someone who could rotate keys (`key:write:any`, Owner or Admin) has left, rotate the org's `admin`-scope keys and update the services that use them. In single-tenant mode, rotate with the master key, so the keys stay the deployment's. Since 2026-11-11 the audit log names the signed-in person behind each key action by email (`actor_label`).

**After upgrading,** list the keys that stopped because their owner left (SQLite; on Postgres write `k.is_active` and `NOT u.is_active`):

```sql
SELECT k.id, k.name, k.owner_user_id FROM api_keys k
LEFT JOIN users u ON u.id = k.owner_user_id
LEFT JOIN organization_members m ON m.user_id = k.owner_user_id
  AND m.org_id = COALESCE(k.org_id, '00000000-0000-0000-0000-000000000001')
WHERE k.is_active = 1 AND k.owner_user_id IS NOT NULL
  AND (u.id IS NULL OR u.is_active = 0 OR m.user_id IS NULL);
```

To get a service going again:

1. Find who owns the key: `SELECT name, owner_user_id FROM api_keys WHERE id = '<key id>';` (the API doesn't show the owner).
2. Make a replacement for the service that used it:
   - Single-tenant: make it with the master key (`POST /v1/keys` with `Authorization: Bearer <master-key>`), so it belongs to the deployment.
   - Hosted: an Owner or Admin of the org makes it, so it belongs to them.
3. Switch the service to the new key, then revoke the old one with `DELETE /v1/keys/{id}`.

---

## Rate limit triggering frequently

**Signals:** Clients receiving 429; `requests_per_minute` near or above key's `rpm_limit` in `/v1/stats`.

1. Check the key's `rpm_limit` at `GET /v1/keys/{id}`.
2. **Option A — raise the limit:** `PATCH /v1/keys/{id}` with `{"rpm_limit": <higher_value>}`.
3. **Option B — multi-key fan-out:** Create additional keys and distribute load across them client-side; the proxy will round-robin provider API keys but the per-key RPM window applies to the single LiteGen key used.
4. Without Redis, rate limiting state is in-process memory: each instance counts for itself, and a restart resets its counts. With `LITEGEN_REDIS_URL` set, all instances draw from the same limits through Redis, and restarting litegen resets nothing. While Redis can't be reached, each instance counts for itself again, or answers 503 `rate_limit_unverifiable` if `LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT` is on. A Redis that keeps answering slowly (250 ms or more a call, `LITEGEN_REDIS_SLOW_CALL_MS`) counts as unreachable too, and so does one that answers `READONLY`, `LOADING` or `MASTERDOWN` (after a failover, during a restart): the log's "Redis answered slowly" warning or the error it names says which. See [scaling.md](scaling.md).

---

## Webhook deliveries failing

**Signals:** Video completions not reaching the subscriber; `success: false` entries in `/v1/keys/{id}/webhook-deliveries`.

1. Fetch delivery history: `GET /v1/keys/{id}/webhook-deliveries`. Check `error_message` and `status_code`.
2. Verify the receiver endpoint is up and returns 2xx for POST requests.
3. Check for signature verification issues: the payload is signed with HMAC-SHA256 of the raw body using the key's stored `key_hash` as the secret. Verify the `X-Litegen-Signature: sha256=<hex>` header on your receiver side.
4. The poller retries up to 3 times with exponential back-off. If all retries fail, the delivery is marked permanently failed — no automatic retry beyond that.
5. To retrigger: cancel and resubmit the generation, or call `POST /v1/keys/{id}/test-webhook` to test the endpoint manually.

---

## Database connection errors

**Signals:** 500 responses across most endpoints; logs show `"Failed to get stats"` or `sqlx` connection errors.

1. Verify `LITEGEN__DATABASE_URL` (or `LITEGEN_DATABASE_URL`, the parts, or `database_url` in the config file) is set correctly. With none, the database is `data/litegen.db` in the working directory (`/app/data/litegen.db` in the Docker image). For SQLite, the file and its directory must be writable by the user litegen runs as; a SQLite database litegen can't open, or can't write when its migrations run, stops it at startup with an error that names the file.
2. For Postgres, confirm network reachability from the LiteGen host/container to the DB host.
3. The connection pool is capped at **10 connections** (see `SqlitePoolOptions::max_connections(10)`). Under heavy load this may be exhausted — check pool wait metrics and consider bumping the limit in source if needed.
4. If using Docker, confirm the `db` service in `docker-compose.yml` is healthy before the app container starts (`depends_on: db: condition: service_healthy`).

---

## OOM in production

**Signals:** Container OOM-killed; process exits with signal 9.

1. Bump container memory limit in `docker-compose.yml` or your orchestrator spec — start with 512 MB, tune from there.
2. Check for stuck materializer temp uploads: reference-image blobs are held in memory during the request. Large batches of base64 images can spike RSS.
3. The generation cache (moka, when `cache.enabled`) is bounded by `cache.max_items` (default 10,000 answers) and by `cache.max_bytes` (default 256 MiB, or `cache_max_bytes` set live in the gateway settings), whichever is reached first. A full cache at its defaults takes about half of a 512 MB container, so `cache.max_bytes` is the knob to lower.
4. Enable `RUST_LOG=debug` briefly to look for allocation-heavy paths, then revert.

---

## Graceful shutdown not happening

**Signals:** In-flight requests are dropped on deploy/restart; video poller leaves orphaned jobs.

1. Verify SIGTERM is reaching the process and not being swallowed by a shell wrapper (`exec` in entrypoint, not `sh -c`). SIGTERM and ctrl-c (SIGINT) both start the graceful stop; Docker sends SIGTERM unless the image sets another `STOPSIGNAL`.
2. What a stop does, in order:
   - litegen stops accepting connections, closes idle keep-alive connections at once, and starts no new poller tick or row;
   - open requests and the poller's current rows get the shutdown grace, `server.shutdown_grace_secs` (unset: the effective vendor timeout + 30 s, 150 s by default), counted from the signal;
   - whatever is still running when the grace ends is cut: the requests' connections are closed, and the rows are polled again by another replica, or by this one after its restart;
   - this replica's poll claims are given back (at most 5 s), so other replicas take its rows at their next tick;
   - the webhooks of jobs that ended before the stop and are still going out, or waiting between attempts, get what is left of the grace. Nothing sends a webhook again, so any still running when the grace ends are abandoned, and the log names their generations. A crash or a `SIGKILL` loses the ones in flight;
   - `LiteGen proxy stopped`.
3. Give litegen that long before the orchestrator kills it: set its grace to `server.shutdown_grace_secs` + 15 s (Compose `stop_grace_period`, Kubernetes `terminationGracePeriodSeconds`), or lower `server.shutdown_grace_secs` to fit. Docker's default is 10 s and Kubernetes' 30 s; the shipped compose files set `stop_grace_period: 165s`.
4. Check the logs, in order: `received the stop signal, shutting down gracefully`; then either `every connection finished; waiting for the poller's current rows` or `the shutdown grace ran out: cut N connections with their requests`; `the shutdown grace ran out while the poller was polling` if the poller was cut; `released this replica's poll claims`; `the shutdown grace ran out with webhook deliveries still running` (with their count and generation ids) if webhooks were abandoned; `LiteGen proxy stopped`. A log that ends after the first line means the orchestrator killed litegen before its grace ended.

---

## Migration failures on upgrade

**Signals:** Process fails to start; logs show `sqlx migrate` error or checksum mismatch.

1. Connect to the DB and inspect the `_sqlx_migrations` table (SQLite) or `sqlx_migrations` table (Postgres) for applied migrations and their checksums.
2. **Never edit migration files retroactively.** If a migration was applied and you need a correction, write a new migration file with the fix.
3. If the migration was not yet applied (e.g., new deployment), check that the binary is the version you meant to deploy: the migrations are built into it (`sqlx::migrate!`), so no migration files need to sit beside it, and a binary only knows the migrations it was built with.
4. Roll back to the previous binary version if the migration is destructive and unintended.

---

## Audit log full

**Signals:** DB disk usage growing unboundedly; `audit_log` table row count very high.

The `audit_log` table grows indefinitely — there is no automatic TTL or rotation. Periodically archive or prune old rows.

**Prune entries older than 90 days (SQLite):**
```sql
DELETE FROM audit_log WHERE created_at < date('now', '-90 days');
```

**Prune entries older than 90 days (Postgres):**
```sql
DELETE FROM audit_log WHERE created_at < NOW() - INTERVAL '90 days';
```

Run `VACUUM` (SQLite) or `VACUUM ANALYZE` (Postgres) after bulk deletes to reclaim disk space. Consider scheduling this as a weekly maintenance job.

---

## Locked-out user can't log in

**Signals:** User reports "too many attempts" or repeated 429 on login; 5 failed attempts within any 15-minute window triggers the lockout.

1. Confirm the lockout by checking the `login_attempts` table for recent rows matching the email.
2. Wait for the 15-minute window to expire, OR have an Admin clear the attempts immediately:

```sql
DELETE FROM login_attempts WHERE email='joe@example.com';
```

3. If the user has also forgotten their password, generate a reset token (see **Forgot password emails not sending** below).

---

## OAuth callback failing

**Signals:** OAuth redirect returns `redirect_uri_mismatch` from the provider; users see an error page after approving the OAuth consent screen.

1. Verify `LITEGEN__OAUTH__CALLBACK_BASE` matches the base URL registered with GitHub or Google exactly (scheme + host + no trailing slash).
2. The callback URLs must be registered with the provider as:
   - GitHub: `<CALLBACK_BASE>/v1/auth/oauth/github/callback`
   - Google: `<CALLBACK_BASE>/v1/auth/oauth/google/callback`
3. A mismatch produces `redirect_uri_mismatch` from the provider — update the registered URL in the provider's OAuth app settings or correct `LITEGEN__OAUTH__CALLBACK_BASE` in the environment.
4. Changes to env vars require a process restart to take effect.

---

## OAuth "account_not_invited"

**Signals:** User successfully authenticates with GitHub/Google but receives `account_not_invited` error.

OAuth sign-in only succeeds for emails already present in the `users` table. OAuth does not auto-create accounts.

1. An Owner or Admin must invite the user via the dashboard (`/users` → Invite) or directly insert a row.
2. The invite email must exactly match the primary email on their GitHub/Google account.
3. After the invitation is created, the user can retry OAuth sign-in.

---

## Proxy refuses to start: "refusing to start … non-loopback"

**Signals:** On boot the process prints `FATAL: refusing to start. Single-tenant mode has no LITEGEN__MASTER_KEY set and is bound to non-loopback address <host> …` and exits with code 1.

**Cause:** In single-tenant mode the no-credential dev bypass grants anonymous requests full admin scope. The startup self-check (`AppConfig::startup_security_check`) refuses to boot that combination on a network-reachable bind so the admin API is never silently exposed. A blank or whitespace-only `LITEGEN__MASTER_KEY` is treated as "no key".

**Fix (pick one):**
1. Set a real master key — `LITEGEN__MASTER_KEY=<a-long-random-secret>` — and restart (recommended).
2. Bind loopback-only — `LITEGEN__SERVER__HOST=127.0.0.1` — behind a reverse proxy.
3. Intentionally run with no auth (trusted network / fronting auth proxy) — `LITEGEN__ALLOW_NO_AUTH=true`; the process starts but logs a loud `SECURITY` warning.

Hosted mode (`LITEGEN__MODE=hosted`) is never blocked by this check — the bypass is disabled there, so unauthenticated requests get `401`.

---

## Owner accidentally locked out

**Signals:** The Owner cannot log in and there is no other Owner (only one Owner exists at a time); `system:transfer_owner` capability is inaccessible.

The Owner is the only role with the `system:transfer_owner` permission. Recovery steps:

1. Set the master key if not already set: add `LITEGEN__MASTER_KEY=<a-long-random-secret>` to the environment and restart the process.
2. Use the master key via Bearer token to find the accounts, with their user ids and roles in the default org (the Members page's list; `GET /v1/users` needs a signed-in Owner or Admin in single-tenant mode):
   ```bash
   curl -H "Authorization: Bearer <master-key>" \
     http://localhost:4000/v1/orgs/00000000-0000-0000-0000-000000000001/members
   ```
3. If the user's password needs resetting, clear `login_attempts` and issue a password-reset token (see **Forgot password emails not sending**).
4. To make an account the Owner, transfer ownership to it with the master key. In single-tenant mode the Owner is recorded twice, in the account's role and in the default org's membership (which holds the permissions), and this call moves both in one transaction. The account must be an active member of the default org:
   ```bash
   curl -X POST -H "Authorization: Bearer <master-key>" -H "content-type: application/json" \
     -d '{"new_owner_user_id": "<user id>"}' \
     http://localhost:4000/v1/orgs/00000000-0000-0000-0000-000000000001/transfer-owner
   ```
   Don't set `users.role` by hand: that changes only one of the two records. To clear a lockout from failed logins, `DELETE FROM login_attempts WHERE email='owner@example.com';`.
5. Remove or rotate `LITEGEN__MASTER_KEY` after recovery is complete.

**At startup, the log warns "the deployment's Owner is recorded differently…"** (target `security`). This means the two records disagree. Transfers made before 2026-11-05 moved only one of them, and a hand edit can do the same. The warning lists the accounts each record names; the membership one holds the Owner's permissions. Run step 4 once for the intended Owner, and the next start is quiet.

---

## Sessions table growing unbounded

**Signals:** DB disk usage growing; `sessions` table row count very high even though users have long since logged out or their sessions expired.

Session rows are not automatically deleted when they expire — they accumulate until pruned. Run a periodic cleanup:

```sql
DELETE FROM sessions WHERE expires_at < datetime('now', '-1 day');
```

Schedule this alongside the existing backup script in cron:

```
# crontab: daily session cleanup (run after backup)
30 0 * * * LITEGEN__DATABASE_URL=... sqlite3 /path/to/litegen.db "DELETE FROM sessions WHERE expires_at < datetime('now', '-1 day');" >> /var/log/litegen-session-cleanup.log 2>&1
```

For Postgres, replace the sqlite3 command with `psql <connection-string> -c "..."`.

---

## Forgot password emails not sending

**Signals:** User requests a password reset but receives no email; SMTP is not configured.

SMTP delivery is out of scope for this version. The password-reset token is surfaced via server logs (`tracing::info!`).

**For development:** set `LITEGEN__DEV__EXPOSE_RESET_TOKENS=true` to include a `_dev_token` field in the password-reset response body, allowing you to retrieve the token without reading logs.

**Manual recovery flow:**
1. Check the server log for a line containing `password_reset_token` and the user's email.
2. Construct the reset URL: `https://your-host/reset-password/<token>`.
3. Send the URL to the user out-of-band (Slack, email client, etc.).
4. The token expires after 1 hour. If it has expired, delete the row from `password_resets` and trigger a new reset request.

---

## Weekly: provider model drift

**Signals:** A vendor shipped a model we don't offer, retired or renamed one we do, or changed a request parameter (sizes, durations, ratios) so our `models/*.yaml` typings no longer match. None of this raises an error until a user hits it.

Run the drift check once a week. See `scripts/model-drift/README.md` for details.

1. In Claude Code, run `/model-drift`. It runs the check, investigates and repairs any broken source (a vendor moved or restructured its docs), then brings back the new, missing and changed items for a decision. Without Claude, run `node scripts/model-drift/check.mjs`.
2. **Exit 2:** a source or definition is broken. The provider's definition in `scripts/model-drift/providers/` needs updating before its results mean anything.
3. **Exit 1:** there is drift.
   - **New:** add the model to `models/<provider>.yaml` with typings verified against the vendor docs, or acknowledge it in the definition with the reason we skip it.
   - **Missing:** confirm against the vendor changelog, then remove or replace the model.
   - **Changed:** `git diff scripts/model-drift/snapshots/` and align the yaml typings.
4. Commit the refreshed snapshots, since they are the baseline for next week's diff.
