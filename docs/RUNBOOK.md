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

1. The circuit breaker should auto-trip after consecutive failures — verify by checking logs for `"circuit breaker opened"`.
2. Once tripped, traffic automatically falls back to the next deployment in the route chain.
3. Check the provider's own status page to determine if it is a transient outage.
4. If you need to force failover immediately, update the model route in `litegen.yaml` / `LITEGEN_CONFIG_FILE` to move the failing deployment to the bottom of the chain (or remove it), then send `SIGHUP` or restart the process.
5. Re-enable when the provider recovers — the circuit breaker resets automatically after the configured open window elapses.

---

## Quota exceeded for a key

**Signals:** Clients receiving 402 or `x-litegen-quota-exceeded: true` header; key holder reports access denied.

1. Fetch the key: `GET /v1/keys/{id}`. Check `tokens_used` vs `token_quota`.
2. **Option A — bump quota:** `PATCH /v1/keys/{id}` with `{"token_quota": <new_limit>}`.
3. **Option B — wait:** There is no automatic monthly reset. Quotas are absolute lifetime usage caps; the admin must manually reset `tokens_used` by patching the DB directly or issuing a new key.
4. If the key should not have a cap, set `token_quota` to `null` via `PATCH`.

---

## Rate limit triggering frequently

**Signals:** Clients receiving 429; `requests_per_minute` near or above key's `rpm_limit` in `/v1/stats`.

1. Check the key's `rpm_limit` at `GET /v1/keys/{id}`.
2. **Option A — raise the limit:** `PATCH /v1/keys/{id}` with `{"rpm_limit": <higher_value>}`.
3. **Option B — multi-key fan-out:** Create additional keys and distribute load across them client-side; the proxy will round-robin provider API keys but the per-key RPM window applies to the single LiteGen key used.
4. Rate limiting state is in-process memory; restart or horizontal scale resets it.

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

1. Verify `LITEGEN__DATABASE_URL` is set correctly. For SQLite, confirm the file path is writable.
2. For Postgres, confirm network reachability from the LiteGen host/container to the DB host.
3. The connection pool is capped at **10 connections** (see `SqlitePoolOptions::max_connections(10)`). Under heavy load this may be exhausted — check pool wait metrics and consider bumping the limit in source if needed.
4. If using Docker, confirm the `db` service in `docker-compose.yml` is healthy before the app container starts (`depends_on: db: condition: service_healthy`).

---

## OOM in production

**Signals:** Container OOM-killed; process exits with signal 9.

1. Bump container memory limit in `docker-compose.yml` or your orchestrator spec — start with 512 MB, tune from there.
2. Check for stuck materializer temp uploads: reference-image blobs are held in memory during the request. Large batches of base64 images can spike RSS.
3. Moka cache is bounded by `max_items` (default 1000) — verify it's not unconfigured and growing unbounded.
4. Enable `RUST_LOG=debug` briefly to look for allocation-heavy paths, then revert.

---

## Graceful shutdown not happening

**Signals:** In-flight requests are dropped on deploy/restart; video poller leaves orphaned jobs.

1. Verify SIGTERM is reaching the process and not being swallowed by a shell wrapper (`exec` in entrypoint, not `sh -c`).
2. The Axum server and the video poller both listen for the shutdown signal. Both should drain within **5 seconds** by default.
3. If using Docker, ensure `STOPSIGNAL SIGTERM` in the Dockerfile and `stop_grace_period: 10s` in Compose.
4. Check logs for `"Shutting down poller"` and `"Server shutdown complete"` to confirm the drain path was reached.

---

## Migration failures on upgrade

**Signals:** Process fails to start; logs show `sqlx migrate` error or checksum mismatch.

1. Connect to the DB and inspect the `_sqlx_migrations` table (SQLite) or `sqlx_migrations` table (Postgres) for applied migrations and their checksums.
2. **Never edit migration files retroactively.** If a migration was applied and you need a correction, write a new migration file with the fix.
3. If the migration was not yet applied (e.g., new deployment), confirm the migration files exist in the `migrations/sqlite` or `migrations/postgres` directory in the binary's working directory.
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

## Owner accidentally locked out

**Signals:** The Owner cannot log in and there is no other Owner (only one Owner exists at a time); `system:transfer_owner` capability is inaccessible.

The Owner is the only role with the `system:transfer_owner` permission. Recovery steps:

1. Set the master key if not already set: add `LITEGEN__MASTER_KEY=<a-long-random-secret>` to the environment and restart the process.
2. Use the master key via Bearer token to call any admin endpoint and bypass user auth:
   ```bash
   curl -H "Authorization: Bearer <master-key>" http://localhost:4000/v1/users
   ```
3. If the user's password needs resetting, clear `login_attempts` and issue a password-reset token (see **Forgot password emails not sending**).
4. If the user row itself needs to be repaired, update it directly:
   ```sql
   UPDATE users SET role='owner' WHERE email='owner@example.com';
   DELETE FROM login_attempts WHERE email='owner@example.com';
   ```
5. Remove or rotate `LITEGEN__MASTER_KEY` after recovery is complete.

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
