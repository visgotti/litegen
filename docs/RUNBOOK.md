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

1. Fetch the key: `GET /v1/keys/{id}`. Check `tokens_used` vs `token_quota`. `tokens_used` includes the prices its requests in flight hold. Right after a replica crashed, part of it may be held by that replica's requests, which is given back within about two and a half minutes ([a replica crashed or was killed](#a-replica-crashed-or-was-killed)).
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
4. A failed attempt is retried after 10 s, 30 s, 1, 2, 5, 10 and 30 min, then 1, 2, 4, 8 and 8 h: 13 attempts, the last about 24 hours after the first. A network error, a timeout and any answer but a 2xx are all retried, except `410 Gone`, which ends the delivery at once. After the last attempt the delivery is `failed`. A delivery's log lines (`Dispatching webhook`, `Webhook non-2xx response`, `Webhook request error`, `Webhook delivered`, `Webhook delivery failed after all retries`, `Webhook delivery failed: the endpoint answered 410 Gone, so it is not retried`, `Webhook delivery failed: it can't be sent`, `Webhook not sent again: an attempt at this job's webhook was already answered 2xx`) name its `generation_id` and its `delivery_id`, the id the receiver got in `X-Litegen-Delivery-Id`.
5. Deliveries wait in the database (`webhook_outbox`, migrations 029 and 032), not in memory: a crash, a kill or a restart loses none. The replica that ended a job sends its first attempt once the job is settled; any replica sends one whose replica died first 30 seconds after the job ended, sends the retries when they are due, and takes over a dead replica's within a minute. See the ones still waiting, and the ones given up, of a key:

   ```sql
   SELECT id, generation_id, status, attempts, next_attempt_at, last_status_code, last_error
   FROM webhook_outbox
   WHERE key_id = '<key id>' AND status IN ('pending', 'failed')
   ORDER BY created_at DESC;
   ```

   A `pending` one is sent again at `next_attempt_at`. A `failed` one is done; a `last_error` with no `last_status_code` says why nothing could be sent (the key's `webhook_url` was cleared, the generation is gone, or the address is one the key may not reach). Unless its generation or key is gone, step 1's list shows the same reason, as an attempt with no `status_code` under its `delivery_id`. To send one that ended (`failed`, or `delivered` to a receiver that lost it) again from the start, once the receiver is back, ask for it with the key's credentials: `POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver` (the `delivery_id` of step 1's rows). It answers `202` with the delivery's state, and a replica sends it within a second under the same delivery id, numbered from attempt 1 again. It answers `409 delivery_pending` while the delivery is still being sent and `429 redeliver_too_soon` within a minute of its last redeliver ([management-api.md](management-api.md#key-webhooks)). For many at once, the same in SQL:

   ```sql
   -- Postgres
   UPDATE webhook_outbox SET status = 'pending', attempts = 0, next_attempt_at = now(), finished_at = NULL,
          last_status_code = NULL, last_error = NULL, redelivered_at = now()
   WHERE key_id = '<key id>' AND status = 'failed';
   -- SQLite
   UPDATE webhook_outbox SET status = 'pending', attempts = 0, next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now'), finished_at = NULL,
          last_status_code = NULL, last_error = NULL, redelivered_at = strftime('%Y-%m-%d %H:%M:%f', 'now')
   WHERE key_id = '<key id>' AND status = 'failed';
   ```

   Setting `redelivered_at` matters: without it a delivery whose job's webhook was answered 2xx once is marked `delivered` again without a request (see the rolling upgrade below).

   A warning `no outbox delivery was recorded for a terminal webhook` means the database lacks the trigger `webhook_outbox_on_terminal` (restored from a dump without it, for instance). Those webhooks go out from memory, as before migration 029, and a crash or a stop can lose them. Re-create the trigger with the last statements of `litegen-core/migrations/<backend>/20240101000032_webhook_outbox_grace.sql` (on Postgres, the function, then the trigger).

   **The log is kept until you set a number.** `webhooks.log_retention_days` is 0 unless set, which keeps every delivery and attempt row for good. Once it is set, the hourly retention sweep deletes a delivery that ended, and each attempt row, that many days after it ended or was made, on the database's clock; a delivery still being sent is never deleted. Step 1's list then shows nothing older, and an older delivery can't be redelivered (404).

   **During a rolling upgrade** from a version before the outbox (before migration 029, dfdfda7 and earlier), the old replicas still send their jobs' webhooks from memory, with no `X-Litegen-Delivery-Id`, while the upgraded replicas' migrations already record every job's delivery, the old replicas' too. An upgraded replica sends such a delivery 30 seconds after its job ended, unless an attempt at that job's webhook was answered 2xx by then: it checks `webhook_deliveries` first and marks the delivery `delivered` with no request (`last_error` says so). So until the last old replica stops, a receiver can get one job's webhook twice, once without a delivery id and once with one, when the old replica's first attempts failed or took longer than 30 seconds. Tell receivers to drop a repeat by generation id and status for the length of the upgrade; after it, the delivery id is enough.
6. Don't resubmit a job to get its webhook again: the new job is billed too, and the outcome is already on the first one. A receiver that was down catches up by polling `GET /v1/generations/{id}` (or `GET /v1/videos/{id}`, `GET /v1/models3d/{id}`) for each id it missed: the ids its own submits returned, or the `generation_id` of the failed rows from step 1. `POST /v1/keys/{id}/test-webhook` only tests the endpoint: it sends one synthetic delivery, signed like a job's webhook, reports the answer and resends nothing.

---

## A deleted org's or app's files

**Signals:** an org or an app was deleted and you want to know its files are gone from litegen's store; or the log says `could not delete a deleted org's or app's file; it is tried again`.

1. Deleting an org or an app (`DELETE /v1/orgs/{id}`, `DELETE /v1/apps/{app_id}`) records each file litegen kept for it in `tenant_file_deletions` (migration 031), in the transaction that deletes its rows: once the delete has answered, a crash, a stop or a store that is down can't lose them. The replica that deleted it starts at once, and every replica takes the due ones every minute (4 at a time, each held for 5 minutes, and four deletes of up to a minute each end within that hold, so two replicas never take one at once). A file deleted is its row deleted, so an empty table means every deleted org's and app's files are gone. Files in an app's own bucket are the customer's and stay. A job that was still delivering when its org or app was deleted stores its files, then finds its row gone: what it stored is recorded here too (log line `a job finished after its org or app was deleted: its files are recorded for deletion`). An image request still running at the delete answers its caller, then removes the rows it wrote after the delete (its generation and request-log rows) and records the files it kept. Every other request removes its rows the same way (its log row, spend tags and artifact): a segmentation, or a request that failed, still answers; a video or 3D job submitted at the delete is cancelled at its vendor and fails with `generation_record_failed`, and a late image ([async-images.md](async-images.md)) answers the `504 vendor_timeout` of a job litegen couldn't record, with no `generation_id`. The delete takes its requests' spend tags too, and on Postgres a tag references its log row (migration 037), so a tag written while the delete runs goes with that row or is refused. The delete itself runs to its end even when its caller hangs up (a proxy's read timeout included): ask again, and a 404 means it went through.
2. See what is left, per deleted org or app (`scope` is `org` or `app`, `tenant_id` its id):

   ```sql
   SELECT scope, tenant_id, COUNT(*) AS files, MAX(attempts) AS attempts,
          MIN(next_attempt_at) AS next_attempt, MAX(last_error) AS last_error
   FROM tenant_file_deletions
   GROUP BY scope, tenant_id;
   ```

3. A row whose `attempts` is above 1 failed before; `last_error` says why (the store's answer, or `the delete took more than 60 s`). It is tried again at `next_attempt_at`: a minute after its first failure, then 2, 4, 8 minutes and so on, up to every 6 hours, for as long as it takes. Once the store is fixed (its credentials, the bucket's policy, the endpoint), make them due now, and a replica deletes them within a minute:

   ```sql
   -- Postgres
   UPDATE tenant_file_deletions SET next_attempt_at = now();
   -- SQLite
   UPDATE tenant_file_deletions SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now');
   ```

4. A URL litegen's store doesn't recognise is let go: nothing is deleted, and its row goes. That is a file in an app's own bucket, a vendor's link (a file litegen didn't keep is never recorded), or a file of litegen's store from before its address changed (`server.public_base_url` for the local store, the bucket or `custom_public_url` for S3). Delete those by hand if they are yours to delete.

---

## A deleted image's copies

**Signals:** a customer deleted an image (`DELETE /v1/generations/{id}`) or relies on an app's `retention_days`, and you want to show no copy of it is left in litegen.

1. Deleting a generation removes its artifact from the request log, and the artifacts of the cache hits it answered (they name it, `output_kind` `generation`, and carry its prompt and params); every log row stays, as the billing record. Retention clears the artifact's output (`output_kind` `deleted`, no `output_value`) and keeps its prompt and params; it clears the copy before it marks the row, so a failure leaves the row due for the next sweep. A cache hit's artifact never holds the image. Migration 040 cleared the copies left before the upgrade. This lists any copy still there; expect none:

   ```sql
   SELECT a.request_id, a.output_kind, g.files_deleted_reason, g.files_deleted_at
   FROM request_artifacts a JOIN generations g ON g.id = a.request_id
   WHERE g.files_deleted_at IS NOT NULL AND a.output_kind IN ('b64', 'url');
   ```

   A row with `files_deleted_reason` `deleted` is a delete that answered 500 (`the files are deleted, but the request could not be removed`): send the same `DELETE` again. To clear every row the query lists at once, run migration 040's statement (`litegen-core/migrations/*/20240101000040_artifact_outputs_of_deleted_files.sql`).
2. The image cache needs nothing: a hit for a generation whose files are deleted, or whose row is gone, is dropped from the cache and answered by the vendor, never served. Copies can still sit in memory, never served: a replica that hasn't looked that request up since the delete keeps its own copy, and so does the shared tier when Redis couldn't be reached to drop it. Each goes at the next lookup of that request, or at the end of the answer's time to live (the gateway's `cache_ttl_seconds`, or its route's) at the latest. `DELETE /v1/cache` drops every cached answer on every replica at once.
3. An `Idempotency-Key` replay is the one way a deleted image still comes back: a request sent with that header stores its whole answer, inline images included, and the same key and request within 24 hours replays it, after a delete too. Stored answers are never replayed once 24 hours old, and the next request with an `Idempotency-Key` deletes them. To drop a deleted generation's stored answers sooner (the answer is stored as its JSON, which holds the generation's id):

   ```sql
   -- Postgres
   DELETE FROM idempotency_keys WHERE position(convert_to('"litegen-img-…"', 'UTF8') IN response_body) > 0;
   -- SQLite
   DELETE FROM idempotency_keys WHERE instr(CAST(response_body AS TEXT), '"litegen-img-…"') > 0;
   ```

---

## Database connection errors

**Signals:** 500 responses across most endpoints; logs show `"Failed to get stats"` or `sqlx` connection errors.

1. Verify `LITEGEN__DATABASE_URL` (or `LITEGEN_DATABASE_URL`, the parts, or `database_url` in the config file) is set correctly. With none, the database is `data/litegen.db` in the working directory (`/app/data/litegen.db` in the Docker image). For SQLite, the file and its directory must be writable by the user litegen runs as; a SQLite database litegen can't open, or can't write when its migrations run, stops it at startup with an error that names the file.
2. For Postgres, confirm network reachability from the LiteGen host/container to the DB host.
3. The connection pool is capped at **10 connections** (see `SqlitePoolOptions::max_connections(10)`). Under heavy load this may be exhausted — check pool wait metrics and consider bumping the limit in source if needed.
4. If using Docker, confirm the `db` service in `docker-compose.yml` is healthy before the app container starts (`depends_on: db: condition: service_healthy`).
5. On Postgres, `canceling statement due to statement timeout` means a statement ran longer than 5 minutes, and `canceling statement due to lock timeout` that it waited longer than 30 seconds for a row or table another transaction held ([statement and lock timeouts](scaling.md#statement-and-lock-timeouts)). For a lock, find the holder: `SELECT pid, state, xact_start, query FROM pg_stat_activity WHERE pid = ANY(pg_blocking_pids(<waiting pid>))`. It is usually a migration on another replica, or a transaction left open in a SQL console. Deleting an org or an app runs without the statement limit, however many rows it has, so it never fails this way; it can still wait 30 seconds for a lock and fail, and is safe to try again (nothing is deleted unless all of it is). A request of that org or app that writes its row while the delete runs (its log row, say) no longer fails it with `violates foreign key constraint`: the delete starts again, up to 5 times, and takes that row too.
6. A billed request whose charge couldn't be written tries again after a few seconds, then every 30 seconds until it lands, its reservation held meanwhile. Each failure logs `a settlement failed again` with its count (`failures`) and the key or limits row (`on`). A stop cuts the retries still going at the end of its grace (counted in `the stop's tracked work … was still running`); the sweep then gives their reservations back, so those requests are charged nothing. `a failed settlement's reservation record is gone ... X USD may be uncharged` names one that may not have been charged. Nothing to repair: reservations left behind are given back by the sweep ([a replica crashed or was killed](#a-replica-crashed-or-was-killed)).

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
   - the webhook dispatcher claims no more deliveries. The attempts already sending get what is left of the grace; every other delivery stays due in the database, for another replica or for this one after its restart, and this replica's claims on the deliveries it isn't sending are given back (at most 5 s). An attempt still sending when the grace ends keeps its claim and is cut with the process; another replica sends its delivery again once that claim lapses (60 s), never while it may still be on the wire (at least once). A crash or a `SIGKILL` loses no webhook either: the deliveries it held pass to another replica once their claims lapse (60 s);
   - the worker that deletes deleted orgs' and apps' files claims no more. The files it held are due again for another replica, or for this one after its restart, once their 5-minute hold passes; one cut mid-delete is simply deleted again ([a deleted org's or app's files](#a-deleted-orgs-or-apps-files));
   - a billed request still running when the grace ends (its client's connection is closed, but the request runs in a task of its own) is cut too, while litegen still runs, so what it reserved on its key's quota and its budgets is given back before litegen exits: at most 5 s past the grace, beside the steps above. A video or 3D submit that has its vendor's answer finishes its row and its charge first. A cut request whose vendor had already taken its job leaves that job running at the vendor with no row ([a replica crashed or was killed](#a-replica-crashed-or-was-killed), item 4);
   - `LiteGen proxy stopped`.
3. Give litegen that long before the orchestrator kills it: set its grace to `server.shutdown_grace_secs` + 15 s (Compose `stop_grace_period`, Kubernetes `terminationGracePeriodSeconds`), or lower `server.shutdown_grace_secs` to fit. Docker's default is 10 s and Kubernetes' 30 s; the shipped compose files set `stop_grace_period: 165s`.
4. Check the logs, in order: `received the stop signal, shutting down gracefully`; then either `every connection finished; waiting for the poller's current rows` or `the shutdown grace ran out: cut N connections with their requests`; `the shutdown grace ran out while the poller was polling` if the poller was cut; `released this replica's poll claims`; `released this replica's webhook delivery claims` (`freed`, and `kept` for attempts still sending); `the shutdown grace ran out with webhook attempts still sending` (with their count and generation ids) if attempts were cut; `the shutdown grace ran out with billed requests still running` (with their count) if requests were cut, and `the stop's tracked work … was still running 5 s after the grace` if some releases or writes didn't finish (a sweep gives back the reservations they held, [a replica crashed or was killed](#a-replica-crashed-or-was-killed)); `LiteGen proxy stopped`. A log that ends after the first line means the orchestrator killed litegen before its grace ended.

---

## A replica crashed or was killed

**Signals:** litegen's log ends without `LiteGen proxy stopped`: an OOM kill, a `SIGKILL` or a `SIGHUP` (which stops litegen at once, with no drain), a lost node, or an orchestrator that killed it before its grace ended. Right after, keys and budgets read as spent on nothing (`tokens_used`, `spend_usd`), and a retry with the same `Idempotency-Key` answers 409 `idempotency_request_in_progress`.

What a crash leaves, and how each part heals by itself:

1. **What its billed requests reserved.** A billed request holds its price on its key's quota and on each budget it counts against until it settles. Each hold is recorded in `reservations` (migration 030) with a lease that the request's replica renews every 20 seconds. After a crash nobody renews them: two minutes later their leases have lapsed, and the next sweep gives the amounts back and deletes the records. Every replica sweeps every 20 seconds and as it starts, so with one replica, its restart sweeps them once their leases lapse. All told, within about two and a half minutes of the crash. Nothing is given back earlier, at a start or otherwise, because another replica's requests may still be running. To see what is held now, and by which replica:

   ```sql
   SELECT holder, target, entity_type, entity_id, amount, expires_at FROM reservations ORDER BY expires_at;
   ```

   A `target` of `api_key` is that key's `tokens_used`, and `limits` the `spend_usd` of the `entity_limits` row (`entity_type`, `entity_id`). Don't delete records by hand: a deleted record's amount is never given back.
2. **Their `Idempotency-Key`s.** A key in progress is held with the same 2-minute lease. Once it lapses, the client's retry runs, as a new request: the dead request's outcome is unknown, and its vendor may have run it. A key recorded before migration 030 has no lease, and its retry waits out 15 minutes from the first request.
3. **Its claims** on jobs, webhook deliveries and deleted tenants' files pass to the other replicas once they lapse: 30 seconds, 60 seconds and 5 minutes ([Graceful shutdown not happening](#graceful-shutdown-not-happening)).
4. **A vendor job with no generation row.** A video or 3D submit, or an image that turned late ([async-images.md](async-images.md)), gets its generation row in the writes that follow its vendor's answer: the request log and its attribution (a late image also stores its request's trace), then the row with the vendor's job id. A crash after the vendor took the job and before that row was written leaves the job running, and billing, at the vendor (on the gateway's account, or on the org's own key), where litegen can neither poll, deliver nor cancel it. Its caller isn't charged (its reservation is given back as in item 1), never learned a generation id, and retries as a new job. The window is those three or four database writes, milliseconds long. A graceful stop lets a video's or a 3D job's writes finish (they run in a task of their own), so its cut lands in the window only for a late image, whose writes run in its request, or in the instant between a video's answer and the start of its writes. To find such jobs, compare the vendor's own list of jobs from the minutes before the crash with the `provider_job_id`s of the generations made then, and cancel strays in the vendor's console. Writing the row before the vendor call would leave only the write of the job id in the window, but litegen mints a generation's id, and on a model route learns which deployment (provider and account) took the job, only once the vendor has answered.
5. **A crash between a submit's row and its charge**, one database write: the job is kept and delivered, but the sweep gives its reservation back instead of charging it, so it costs its caller nothing, and if it fails later, its refund credits its price once more.

---

## Migration failures on upgrade

**Signals:** Process fails to start; logs show `sqlx migrate` error or checksum mismatch.

1. Connect to the DB and inspect the `_sqlx_migrations` table (on SQLite and Postgres alike) for applied migrations and their checksums.
2. **Never edit migration files retroactively.** If a migration was applied and you need a correction, write a new migration file with the fix.
3. If the migration was not yet applied (e.g., new deployment), check that the binary is the version you meant to deploy: the migrations are built into it (`sqlx::migrate!`), so no migration files need to sit beside it, and a binary only knows the migrations it was built with.
4. **Rolling back means restoring the database backup taken before the upgrade.** A binary refuses to start on a database that has applied a migration it doesn't contain (sqlx: `migration 20240101000027 was previously applied but is missing`), so an older version won't start on an upgraded database. Restoring the backup loses what changed since the upgrade. The same goes for a rolling upgrade: an old replica keeps running, but once a new one has migrated, an old one that stops can't start again. Finish the rollout, or restore.
5. **A large `request_artifacts` on Postgres (migration 039).** 039 builds a partial index on `request_artifacts` inside the migration's transaction. Not `CONCURRENTLY`: under sqlx's migration lock that deadlocks with a replica that boots meanwhile (see 023's header). While it builds, writes to that table (one per request) wait: one pass over the table. On tens of millions of rows that can take minutes, longer than the 30 s a request waits for a lock. To avoid the wait, build the index yourself before upgrading, while the old version runs:

   ```sql
   CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_request_artifacts_served_generation
       ON request_artifacts (output_value) WHERE output_kind = 'generation';
   SELECT indisvalid FROM pg_index WHERE indexrelid = 'idx_request_artifacts_served_generation'::regclass;
   ```

   Run each statement on its own, as psql does when you type them one at a time; `CONCURRENTLY` refuses to run in a transaction, and two statements sent together count as one. Writes go on while it builds. If `indisvalid` is `f`, the build was cancelled or failed: drop the index with `DROP INDEX CONCURRENTLY idx_request_artifacts_served_generation;` and build it again. Never leave an invalid one in place: 039's `IF NOT EXISTS` would keep it, and the deletes it serves would read the whole table. Once the index is valid, the upgrade's 039 finds it and does nothing.

---

## Reservations recorded but not taken (SQLite)

**Signals:** the startup error `the database lacks migration 030's trigger reservations_take`; requests are charged less than they cost, and budgets and quotas don't hold requests in flight.

On SQLite a billed request's reservation is taken by the trigger `reservations_take` as its record is inserted (migration 030). A database restored without the trigger records each reservation without taking it, so each request settles to its cost less its estimate, about nothing. Postgres uses no trigger for this. Re-create it with the `CREATE TRIGGER IF NOT EXISTS reservations_take` statement at the end of `litegen-core/migrations/sqlite/20240101000030_reservations.sql`, run in `sqlite3` against the database file; it takes effect at once, with no restart. Charges already settled without it stay as they were.

---

## Database growing: what grows, and how to bound it

**Signals:** the database file (SQLite) or database (Postgres) keeps growing; `request_logs` or `request_artifacts` holds millions of rows.

What grows with traffic, and what bounds it:

| Table | Holds | Bounded by |
|---|---|---|
| `request_artifacts` | each request's prompt, params and output, up to 2 MiB (an image answered inline); by far the heaviest | `logs.artifact_retention_days`, or else `logs.retention_days`; kept for good unless set |
| `request_logs`, `request_log_tags` | a row per request (model, cost, status, latency, who it's billed to) and its spend tags | `logs.retention_days`; kept for good unless set |
| `generations` | a row per job: its status, cost, assets and request | Not deleted by any retention. `storage_access.retention_days` deletes the files a job kept; the row stays, as the billing record |
| `webhook_outbox`, `webhook_deliveries` | each webhook delivery and attempt | `webhooks.log_retention_days`; kept for good unless set |
| `audit_log`, `sessions` | management changes; dashboard sign-ins | nothing: prune them by hand ([Audit log full](#audit-log-full), [Sessions table growing unbounded](#sessions-table-growing-unbounded)) |

**Bound the request log** in `litegen.yaml`, then restart:

```yaml
logs:
  retention_days: 90            # each log row, with its spend tags; 0 keeps them (the default)
  artifact_retention_days: 14   # each artifact, sooner; 0 follows retention_days
```

The variables are `LITEGEN__LOGS__RETENTION_DAYS` and `LITEGEN__LOGS__ARTIFACT_RETENTION_DAYS`. Each takes 0 to 36500 days. litegen refuses to start when artifact days exceed a nonzero `retention_days`, naming both keys.

- **What happens.** Every replica's retention sweep runs 5 minutes after start, then hourly. Each round deletes, on the database's clock, the artifacts and log rows older than their retention, 100 artifacts and 5000 log rows a batch. It goes on while batches come back full, for up to 10 minutes. A large backlog, as on the first start with a retention on an old database, is worked off over several rounds. Each round that deleted something logs `retention sweep: deleted the request log past its retention`, with `logs=` and `artifacts=` counts. Two replicas purging at once is harmless.
- **SQLite.** It takes one writer at a time, so this purge and the webhook log's wait 150 ms after each batch. Requests that write their log row meanwhile go in between batches instead of timing out (`database is locked`).
- **What you lose.** `GET /v1/logs`, `GET /v1/stats`, `GET /v1/spend/report` and `GET /v1/spend/logs` read only the rows kept, and so do the dashboard's Overview, Logs and Spend pages. Export the spend you need before setting a retention shorter than your reporting period. `GET /v1/logs/{id}/artifact` answers 404 once the artifact is gone. Budgets and key quotas keep their own running totals, so they aren't affected, and generations aren't deleted.
- **Disk space.** Deleted rows free pages that new rows reuse, so once the backlog is gone the database levels off at about the size of its kept window, but its size on disk doesn't shrink. On SQLite, `VACUUM` gives the space back. It rewrites the whole file, needs that much free disk again, and holds writes until it finishes, so run it in a quiet hour after the backlog is gone. On Postgres, autovacuum makes the space reusable; `VACUUM FULL request_artifacts` gives it back but locks the table while it runs.

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

## Operator entries in the default org's audit log

**Signals:** A hosted deployment ran a version from before 2026-11-15, and its default org has members besides you.

The master key acts in no org. Those versions filed its changes to the gateway and to accounts under the default org (`provider.configure`, `provider.reset`, `settings.update`, `links.revoke`, `routes.update`, `routes.reset`, `model.*`, `user.update`, `user.delete`, `system.transfer_owner`), and the password change of an account that belongs to no org too (`account.change_password`). That org's Owners and Admins read them on the Audit page and in its CSV export. Now these entries are filed under no org, and only the master key reads them (`GET /v1/audit` with the master key; [management-api.md](management-api.md#audit-log)).

1. Upgrade. Each start in hosted mode moves the master key's old gateway and account entries out of the default org's log, and logs how many it moved (`audit log: moved the master key's gateway and account entries out of the default org's log`). It moves an entry for one of the actions above, `account.change_password` aside, that has no `actor_key_id` and `master-key` as its `actor_label`. In hosted mode only the master key takes those actions, so each entry it moves is the master key's. Once none are left, a start changes nothing.
2. The master key's changes to an org's own objects (a budget, a key, an end user, a member) were always filed under that org, and they stay there: its admins keep seeing them.
3. Old `account.change_password` entries of people who belonged to no org stay in the default org. The entry names the person by email, and nothing in it tells whether they acted in an org then. To review them, list the ones whose person is not a member of the default org now:
   ```sql
   SELECT id, actor_label, created_at FROM audit_log
   WHERE org_id = '00000000-0000-0000-0000-000000000001'
     AND action = 'account.change_password'
     AND actor_label NOT IN (
       SELECT u.email FROM users u
       JOIN organization_members m ON m.user_id = u.id
       WHERE m.org_id = '00000000-0000-0000-0000-000000000001');
   ```
4. During a rolling upgrade, a replica still on the old version files new entries under the default org. Restart one replica once every replica runs the new version: its start moves them.
5. Backups and CSV exports made before the upgrade still hold the entries.

---

## Provider header tokens in old audit entries

**Signals:** A provider's `extra_headers` held a token (a gateway's `cf-aig-authorization` or `Helicone-Auth`, say), set through `PUT /v1/admin/providers/{name}` or the dashboard's Providers page on a version from before 2026-11-14.

Before then, the change's `provider.configure` audit entry kept the header values, and `GET /v1/audit` showed them, as did the dashboard's Audit page and its CSV export. In hosted mode the master key's entries were filed under the default org until 2026-11-15 ([Operator entries in the default org's audit log](#operator-entries-in-the-default-orgs-audit-log)), so that org's Owners and Admins could read the tokens. In single-tenant mode the audit log's readers can change the providers anyway. Now an entry keeps each header's name, with `[redacted]` for its value ([provider-config.md](provider-config.md#extra_headers)).

1. Upgrade. Migration 027 writes `[redacted]` over the values in the stored entries and changes nothing else.
2. In hosted mode, if the default org has members besides you, treat each such token as exposed: rotate it at the gateway or vendor, then set the new one with `PUT /v1/admin/providers/{name}`.
3. Backups and CSV exports made before the upgrade still hold the tokens. So do database backups, and the database itself, while `encrypt_provider_credentials` is off: `provider_configs.settings_json` keeps header values as set ([Provider credentials at rest](#provider-credentials-at-rest)).
4. During a rolling upgrade, a replica still on the old version writes values again. Change provider headers once every replica runs the new version, or run migration 027 by hand afterwards (`litegen-core/migrations/sqlite/` or `postgres/`, `20240101000027_audit_header_values.sql`): it changes only entries that still hold a value.

Headers set in `litegen.yaml` or in `LITEGEN__PROVIDERS__…` variables never reached the audit log.

The budget alert webhook went the same way: each `settings.update` entry kept the whole `alert_webhook_url`, and a webhook URL can carry its secret in the path (Slack, Discord) or in the host (Pipedream, an AWS Lambda function URL). Now an entry shows `[redacted]` for it, and migration 028 rewrites the stored entries the same way; `GET /v1/admin/settings` still shows the whole URL. In hosted mode, make a new URL at the receiving service and save it with `PUT /v1/admin/settings`.

An `api_base` with user info (`https://user:pass@proxy.example/v1`) was shown whole by the provider view and kept whole in `provider.configure` entries. Now both show `https://[redacted]@proxy.example/v1`, and the same holds for a URL with user info in `options`. Entries written before still hold them: in hosted mode, change those passwords as in step 2.

---

## Provider credentials at rest

**Signals:** A start logs that `encrypt_provider_credentials` is off, or names runtime provider configs that keep credentials in plain text, or that keep encrypted ones while it is off. Or a rollback to a version from before 2026-11-15 is coming.

Runtime provider configs keep their extra header values and the user info of their URLs (`api_base`, URLs in `options`) in plain text in `provider_configs.settings_json` until the gateway setting `encrypt_provider_credentials` is on ([provider-config.md](provider-config.md#credentials-at-rest)). Then they are encrypted with the config's keys, and a version from before 2026-11-15 can't read the rows: it sends `[redacted]` in place of each value. A start never changes the rows. Only a save of the gateway settings does, on the replica that takes it, so replicas on two versions never undo each other.

To turn it on:

1. Set the same `LITEGEN__SECRETS_KEY` on every replica.
2. Upgrade every replica to this version. A single replica can go straight to step 3.
3. Read the settings (`GET /v1/admin/settings`) and save its `settings` back with `"encrypt_provider_credentials": true` (`PUT /v1/admin/settings`). The `PUT` clears the other fields it isn't sent; this one, left out, keeps its stored value. The replica that takes the save encrypts every stored row. Its log line (`runtime provider configs stored again as encrypt_provider_credentials says`) and the `provider.encrypt_credentials` audit entry say how many rows, and which.

A version from before 2026-11-15 won't start on this database at all: it refuses the migrations it doesn't know (see **Migration failures on upgrade**, step 4). So a rollback means restoring the database backup taken before the upgrade, which holds the rows as older versions read them.

Only if you must keep this database and start an older binary on it, by deleting the newer rows from `_sqlx_migrations` (unsupported; the next upgrade can't re-apply those migrations cleanly, so restore the backup before upgrading again):

1. Save the settings with `encrypt_provider_credentials` set to `false` or `null` (a save that leaves it out keeps it on). The replica that takes the save stores every row as older versions read it (a `provider.decrypt_credentials` entry). A 500 means some rows weren't stored again: send the same save again, which changes only what is left.
2. Run the deletes of **Reset and invitation links across the 041 upgrade**, step 4, and of **Sessions across the 044 upgrade**, step 3.
3. Delete the newer migrations' rows from `_sqlx_migrations` and start the older version. Don't turn the setting on again until every replica runs this version.

A row whose keys don't decrypt with the replica's secrets key can't be stored again either way. A warning names it, and no version can use it until its keys are saved again. Database backups taken while the setting was off hold the values in plain text.

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

In single-tenant mode OAuth makes no account on its own: it signs in to an account that is already linked to the provider, or accepts an invitation.

1. An Owner or Admin (or the master key) invites the user: the dashboard (`/users` → Invite), or `POST /v1/orgs/{id}/members`. LiteGen sends no email: the answer carries the invitation's link (`invite_url`, or `/invite/<token>` on the dashboard), which they send to the user. A `users` row inserted by hand doesn't work: OAuth links a new identity only through an invitation, or onto an account already signed in with GitHub or Google.
2. The invite email must exactly match the primary email on their GitHub/Google account.
3. The user opens the link and chooses **Sign in with GitHub** or **Google** there. A lost link can't be shown again: revoke the invitation (`DELETE /v1/orgs/{id}/invitations/{invitation_id}`; `GET /v1/orgs/{id}/invitations` lists them) and invite again.

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
2. Use the master key via Bearer token to find the accounts, with their user ids and roles in the default org (the Members page's list; `GET /v1/users` lists every account with its own role, and takes the master key too since 2026-11-15):
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

**Signals:** User requests a password reset but receives no email.

LiteGen sends no email. The self-service request (`POST /v1/auth/password-reset/request`) always answers `{"ok": true}`, at the same speed whether or not the email has an account, and stores, logs and delivers nothing (unless `dev.expose_reset_tokens` is on, for local tests). A reset link comes from an admin who outranks the account, or from the master key.

**Recovery flow:**
1. Find the account's id: `GET /v1/users` with the master key (or as the deployment's Owner or an Admin), or the org's Members page.
2. **If the account may be in someone else's hands** (a stolen password or session, a lost laptop), deactivate it first: `PATCH /v1/users/<user id>` `{"is_active": false}`, or `DELETE /v1/users/<user id>`. In one step that:
   - ends its sessions, and revokes its apps' OAuth sign-ins and the codes they haven't redeemed yet;
   - deletes its unused reset links, and the ones it issued;
   - revokes the pending invitations it sent, in every org (each an `invitation.revoke` audit entry with `"reason": "inviter_deactivated"`);
   - moves its sign-in epoch on, so a sign-in still in flight (a login checking the password, an app's approval or code exchange) is refused when it lands, and so is a reset link or an invitation it was sending (`401 sign_in_ended`, nothing stored).

   Every API key the account owns stops at once, since a key whose owner is inactive is refused: that outruns anyone minting new keys from a key or a session. Then, while it is deactivated:
   - **Block its API keys** with the master key, each one: `POST /v1/keys/<key id>/block`. They would work again once the account is reactivated; a blocked key stays blocked through the reactivation, until someone unblocks it (`POST /v1/keys/<key id>/unblock`). The master key may block a key in any org. Don't use `DELETE /v1/keys/<key id>` here: it acts only in the org the caller acts in. For the master key that is the default org in single-tenant mode, where a key in any other org answers `404` as if it were gone, and no org in hosted mode, where every key answers `403`. A stolen session makes keys in every org the account belongs to, including any it created. The database lists all the account owns, in every org: `SELECT id, org_id, name FROM api_keys WHERE owner_user_id = '<user id>' AND is_active;`.
   - **Check the audit log in each org the account belongs to**, with the master key: `GET /v1/audit?org_id=<org id>`. Without `org_id` it reads one org only (the default org in single-tenant mode; in hosted mode only the entries filed under no org). The orgs: `SELECT org_id FROM organization_members WHERE user_id = '<user id>';`. Look from when the account may have been taken until a moment after the deactivation: a write in flight when the deactivation committed can land just after it. Look for:
     - members it added, invited or changed (`org.member.*`, `invitation.create`, `user.update`);
     - keys it made (`key.create`);
     - links it issued (`user.password_reset_link`);
     - changes to the gateway: provider settings and credentials (`provider.configure`, `provider.reset`, `org.provider_credential.upsert`), gateway settings (`settings.update`) and app access (`app.access.update`). A provider's `api_base` can be pointed at any address, which is then sent that provider's key with every request: set it back, and rotate that vendor key with the vendor.

     Remove or revoke what you don't recognize.

   In hosted mode the master key may deactivate any account, a tenant's signup (an org Owner) included: ownership is each org's, not the account's. The single-tenant deployment's Owner can't be deactivated; for that account see **A compromised Owner** below.
3. Issue a link: `POST /v1/users/<user id>/password-reset`.
   - The master key may for any account, and is the only way for the deployment's Owner.
   - A person may only for an account they outrank in every org it belongs to, as an Owner or Admin there (an Admin for Members and Viewers, an Owner for Admins too), and never for their own account: they change their password with `PATCH /v1/account`.
   - For a deactivated account, only someone who may reactivate it: in single-tenant mode a person who outranks it in the deployment's org, and in either mode the master key. A link from before the deactivation is gone, so the link must be issued after it.
   ```bash
   curl -X POST -H "Authorization: Bearer <master-key>" \
     http://localhost:4000/v1/users/<user id>/password-reset
   ```
4. The answer carries the `token` and, when `server.public_base_url` is set, `reset_url` (the dashboard's `/reset-password/<token>` page). It is shown only once. Send it to the person out of band (Slack, your own email).
5. They set a new password at that page, or with `POST /v1/auth/password-reset/confirm` `{"token": "...", "new_password": "..."}`. The token works once, for 1 hour. For an expired one, issue another: a new link voids the account's older unused ones.
6. Setting the password reactivates a deactivated account, and ends every sign-in again in the same step: its sessions, its apps' OAuth sign-ins and their unredeemed codes, with the sign-in epoch moved on. Its API keys work again, so whenever someone else may have held the account, block them while it is still deactivated (step 2): a stolen session makes keys as easily as a stolen password.
7. Confirming also checks, in the same step, that whoever issued the link may still issue it: still active, still above the account in every org it belongs to, and, for a deactivated account, still allowed to reactivate it. Otherwise it answers `400 invalid_token` ("The link's issuer may no longer issue it") and changes nothing: issue a new link. The master key's links always qualify. An issuer's unused links are also deleted as soon as the issuer is deactivated, removed from an org, or has a role changed.

**A compromised Owner (single-tenant).** The deployment's Owner can't be deactivated, so move ownership out of the way first, then deactivate the old Owner. Do this whether password sign-in is on or off. A reset of the Owner's password alone isn't enough: it leaves the account's API keys working, and an admin-scope key can make new admin-scope keys faster than they can be revoked one at a time. All the calls below use the master key.
1. **Pick who holds ownership meanwhile**: another active member of the default org (`GET /v1/orgs/00000000-0000-0000-0000-000000000001/members`). If the Owner is the only account, make a temporary one:
   1. Invite it: `POST /v1/users` `{"email": "<an address you control>", "role": "admin"}`.
   2. Accept it with the answer's `token`: `POST /v1/auth/invitations/<token>/accept` `{"password": "<long random>"}`. This works with password sign-in off too. The account only receives ownership and never needs to sign in.
2. **Transfer ownership to them.** Each route takes its own body:
   ```bash
   curl -X POST -H "Authorization: Bearer <master-key>" -H "content-type: application/json" \
     -d '{"new_owner_user_id": "<their user id>"}' \
     http://localhost:4000/v1/orgs/00000000-0000-0000-0000-000000000001/transfer-owner
   # or
   curl -X POST -H "Authorization: Bearer <master-key>" -H "content-type: application/json" \
     -d '{"new_owner_id": "<their user id>"}' \
     http://localhost:4000/v1/users/transfer-owner
   ```
   The old Owner is now an Admin.
3. **Deactivate the old Owner**: `DELETE /v1/users/<old owner id>`. In one step that ends its sessions, OAuth sign-ins and unredeemed codes, deletes its unused reset links and the ones it issued, and revokes the pending invitations it sent. It also moves its sign-in epoch on, so a sign-in, a link or an invitation still in flight is refused. Every API key the account owns stops at once (a key whose owner is inactive is refused), which outruns a thief minting new keys from a key or a session.
4. **Block its API keys while it is deactivated**, with the master key, in every org: `POST /v1/keys/<key id>/block` for each key the step-2 SQL of the recovery flow above lists. They would work again once it is reactivated, and a block outlasts the reactivation.
5. **Check the audit log in each org the account belongs to** (`GET /v1/audit?org_id=<org id>` with the master key), for the entries step 2 of the recovery flow above lists, gateway changes included, from when the account may have been taken until a moment after the deactivation. Remove or revoke what you don't recognize.
6. **Reactivate the account** once the person has their sign-in back. Reactivating moves its sign-in epoch on, so nothing made while it was deactivated comes back.
   - With password sign-in on, issue it a link while it is still deactivated: `POST /v1/users/<old owner id>/password-reset`. Hand the link over. Their confirm sets the new password and reactivates the account in one step. Don't reactivate it first with `PATCH`: the old password would work again until they set a new one.
   - With password sign-in off (`auth.allow_password: false`, which refuses a confirm with `password_auth_disabled`), reactivate it with `PATCH /v1/users/<old owner id>` `{"is_active": true}`. They sign in with GitHub or Google, once they hold those accounts again.
7. **Transfer ownership back** to them once the account is active again, with the call from step 2 and their id.
8. **Restore the temporary Owner's role.** Transferring ownership leaves the account that held it an Admin. Set its earlier role explicitly: `PATCH /v1/users/<their user id>` `{"role": "member"}` (or `"viewer"`; nothing to do if it was an Admin). If you made a temporary account in step 1, deactivate it instead: `DELETE /v1/users/<its id>`.

Each link issued (`user.password_reset_link`) and each password it set (`user.password_reset`, with `"reactivated": true` when it brought the account back) is in the audit log, under each organization the account belongs to. Changing the account's roles, ownership or memberships deletes its unused links (migration 042's triggers), since the link was checked against the roles at the time, and so does deactivating it, also when it was already deactivated: issue a new one after such a change. A person who changes their own password (`PATCH /v1/account`) ends their other sign-ins the same way, and deletes the links they issued, keeping the session they changed it from. The invitations they sent stay pending, so a routine password change doesn't void real ones: someone who changes their password because a session was stolen should review them on the Members page (or `GET /v1/orgs/{id}/invitations`) and revoke any they didn't send (`DELETE /v1/orgs/{id}/invitations/{invitation_id}`). The change writes only while nothing moved since it checked the current password: a reset, a deactivation or another change that committed meanwhile makes it answer `401 sign_in_ended`, with nothing written, so a thief looping password changes can't overwrite a reset. LiteGen keeps only a hash of each token, so a lost link can't be shown again: issue another.

**For development:** `dev.expose_reset_tokens: true` (or `LITEGEN__DEV__EXPOSE_RESET_TOKENS=true`) makes the self-service request answer with the token as `_dev_token`. That hands any account to whoever knows its email, so it is for local tests only: see **Refuses to start: a dev token flag**.

---

## Refuses to start: a dev token flag

**Signals:** On boot the process prints `FATAL: refusing to start. dev.expose_reset_tokens (LITEGEN__DEV__EXPOSE_RESET_TOKENS) … the public could reach this server: …` (or the same for `dev.expose_invite_tokens`) and exits with code 1.

**Cause:** The dev token flags are for local tests only. `dev.expose_reset_tokens` answers an unauthenticated reset request with the reset token, so anyone who knows an account's email could take it over. LiteGen refuses to start with either flag on in hosted mode, on a bind other than loopback, or with a `server.public_base_url` whose host isn't loopback (a proxy on the same host forwards public traffic to 127.0.0.1). On a loopback-only single-tenant server it starts, and logs a `SECURITY` warning naming the flag.

**Fix:** Turn the flag off. To hand someone a reset link, issue one with `POST /v1/users/<user id>/password-reset` (see **Forgot password emails not sending**). An invitation's answer carries its token without any flag.

---

## Startup warning: whoever signs up first becomes the Owner

**Signals:** On boot the log warns `SECURITY: no account exists yet and LITEGEN__OWNER_EMAIL is not set, so whoever signs up first becomes the Owner, and the public could reach this server: …`.

**Cause:** A single-tenant deployment with no account lets its first signup claim the Owner. The warning appears when the server binds an address other than loopback, or has a `server.public_base_url` that isn't loopback.

**Fix:** Set `LITEGEN__OWNER_EMAIL` (or `owner_email` in the config file) to the Owner's address before you expose a fresh deployment, and restart. Then only that address can sign up first. Or sign up yourself before anyone else can reach it. Once an account exists, the warning stops.

---

## Reset and invitation links across the 041 upgrade

**Signals:** After the upgrade, a reset link handed out shortly before it answers `invalid_token`. During a rolling upgrade, an invitation or reset link fails on some tries and works on others.

Migration 041 (2026-11-15) keeps only a SHA-256 hash of each reset and invitation token, so a copy of the database holds no token that works.

1. Reset links issued before the upgrade stop working: 041 deletes the stored ones (they lived an hour anyway). Issue them again.
2. Invitations made before the upgrade keep their plain token, and their links work until they expire, 7 days after they were made. Nothing to do.
3. While old and new replicas run side by side, the link a new replica hands out fails on an old one (`invitation_not_found`, `invalid_token`), and neither can confirm a reset link that the other issued. A new replica accepts invitations that old ones made. A failed try uses nothing up, so the same link works once it reaches a new replica.
   Finish the rollout before handing out links, for a second reason too: an old binary looks a token up as it is stored, so the stored hash itself works as the token there. Anyone with a copy of the database could accept a new invitation, or take an account over through an unused reset link, on an old replica.
   An old replica keeps running during the rollout, but one that stops after a new one migrated can't start again (see **Migration failures on upgrade**, step 4).
4. **Rolling back** means restoring the database backup taken before the upgrade: an older binary won't start on this database. The restored database holds no hashed tokens, so nothing more is needed.
   Only if you keep this database and delete the newer rows from `_sqlx_migrations` to start an older binary (unsupported; the next upgrade can't re-apply those migrations cleanly): first delete what the old binary would take a stored hash for, the unused reset links and the unused invitations made since 041 (the ones with a `token_hash`). Accepted invitations can't be accepted again on any version, so they can stay.
   ```sql
   DELETE FROM password_resets WHERE used_at IS NULL;
   DELETE FROM invitations WHERE token_hash IS NOT NULL AND used_at IS NULL;
   ```
   Then issue the links again on the old version. The triggers of 042 and 043 stay; they only delete unused reset links.

---

## Sessions across the 044 upgrade

**Signals:** After the upgrade, everyone is signed out of the dashboard once. During a rolling upgrade, a session works on some requests and gets `401 Invalid session` on others.

Migration 044 (2026-11-16) keeps only a SHA-256 hash of each dashboard session's token, so a copy of the database holds no session that works. The `litegen_session` cookie carries the token, and `sessions.id` holds its hash.

1. 044 deletes every session stored before it, since those held the plain token and SQLite can't hash them in place. Everyone signs in again once. Accounts, API keys, apps' OAuth sign-ins (MCP clients) and pending links stay.
2. While old and new replicas run side by side, a session made on one version gets `401` on the other. Sign in again once the rollout is done. Finish the rollout quickly, for a second reason too: an old binary looks a session up as it is stored, so the stored hash itself works as the cookie there. Anyone with a copy of the database could use a new session on an old replica.
3. **Rolling back** means restoring the database backup taken before the upgrade: an older binary won't start on this database (see **Migration failures on upgrade**, step 4).
   Only if you keep this database and delete the newer rows from `_sqlx_migrations` to start an older binary (unsupported): first delete every session, since the old binary would take a stored hash for a cookie. Everyone signs in again on the old version.
   ```sql
   DELETE FROM sessions;
   ```

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
