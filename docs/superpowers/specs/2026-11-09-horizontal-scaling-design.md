# Horizontal scaling with Redis: design

Date: 2026-11-09. Status: approved by the user on 2026-11-09.

Who decided what: **(user)** marks a decision the user made. **(my call)**
marks one made in writing this design, which the user can overturn.
Anything unmarked follows from those two or from how litegen already works.

## Why

The user wants this outcome: once a Redis URL is configured, litegen runs as
any number of replicas behind a plain load balancer, with no sticky sessions
and no per-replica settings. API key rate limits, and every other limit or
piece of state that has to be shared, stay in sync across replicas at all
times. **(user)**

Today litegen has no Redis code at all, so setting a Redis URL does nothing.
The README tells operators to pin each API key to one instance with sticky
sessions ([README.md:683-686](../../../README.md)). The multi-tenant design
left this as "Phase 3"
([2026-06-03 spec, line 285](2026-06-03-multi-tenant-hosted-platform-design.md)),
and nobody built it.

The prior art is LiteLLM's proxy. Match how it behaves and how it names its
settings (with a `LITEGEN_` prefix and one underscore), unless this document
says otherwise. **(user)**

## Where things stand (audit, 2026-11-09)

### Already shared, through the database

These work across replicas as long as the replicas share one Postgres. Leave
them alone.

- **Budgets and key token quotas:** conditional `UPDATE`s
  (`reserve_limits_spend` in `management/sql.rs`, `reserve_tokens` in
  `db/postgres.rs`).
- **API keys:** the database is read on every request
  (`lookup_api_key_by_hash`), so a revoked key stops working on every replica
  at once.
- Login lockout, idempotency keys and link revocation.
- **MCP:** it keeps no sessions (`NeverSessionManager`), and its OAuth grants
  live in the database. Dashboard OAuth state lives in cookies.
- **Job outcomes:** `update_generation_if_active` lets one replica decide each
  outcome, so its webhook goes out once. 3D deliveries hold a database lease
  (`proxy/delivery_lease.rs`), and stages are claimed on the row. A replica
  that didn't submit a job answers from the job's row.
- **Gateway settings and model routes:** every replica reloads them every
  15 s (`management/runtime.rs`, `refresh`).

### Kept separately on each replica, which breaks with several

With N replicas behind a round-robin balancer:

| What | What goes wrong | Where |
|---|---|---|
| Per-minute rate limits. One `RateLimiter` serves the key-level `rpm_limit` and the rpm of every principal (key, org, app, member, end user) | A key gets up to N × its RPM | `api/middleware/rate_limit.rs`, `api/middleware/mod.rs:617`, `management/enforce.rs:457` |
| Parallel-request slots | Up to N × the cap | `management/enforce.rs:99` (`IN_FLIGHT`) |
| The MCP sign-in limit per address | Up to N × the limit | `mcp/oauth/limit.rs` (`AddressLimiter`) |
| Runtime models (add, replace, patch, delete, disable) and provider configs | Only the replica that handled the call applies them. The others apply them at their next boot | `api/handlers/management/admin.rs:101,203,231,372,403`, `management/runtime.rs` (`apply_at_boot`) |
| The provider circuit breaker | Each replica has to see its own threshold of failures | `proxy/circuit_breaker.rs` |
| The image response cache (moka) | One cache per replica, and `DELETE /v1/cache` clears only one | `proxy/cache.rs`, `api/handlers/mod.rs:4307` |
| The poller | Every replica polls the same 100 oldest active rows every 5 s. Vendors get N × the status calls, and polling capacity doesn't grow with replicas | `proxy/poller.rs:129`, `list_active_generations` |
| The link-signing key, when `LITEGEN__SECRETS_KEY` isn't set | Each process picks a random key, so a link signed on A fails on B | `access/signing.rs:39` |

### Kept separately on each replica, on purpose

Leave these alone: the process-wide in-flight cap (`InFlightLimit`, which is
backpressure for one process), the latency samples behind lowest-latency
routing, the AWS STS and JWKS caches, and the cursors for provider key pools.

## Goals

1. **Connection settings** named as in [Configuration](#configuration).
   **(user)**
2. **Shared limits and state when Redis is set:** rate limits, parallel
   slots, the MCP sign-in limit, the provider circuit breaker and the image
   cache are shared by every replica.
3. **Admin changes reach every replica:** within about a second with Redis,
   and within 15 s without it.
4. **The poller:** each active job is polled by one replica at a time, and
   polling capacity grows with the replica count.
5. **One link-signing key for all replicas**, with nothing extra to
   configure.
6. **A Redis outage** behaves the way LiteLLM's does
   ([Redis outage](#redis-outage)). **(user)**
7. **Proof:** an automated test runs two replicas against one Postgres and
   one Redis.

## Non-goals

- Redis Cluster and Sentinel. LiteLLM has `REDIS_CLUSTER_NODES` and
  `REDIS_SENTINEL_NODES`, which can come later.
- Moving budgets, spend or anything else the database already shares into
  Redis. Postgres stays the source of truth.
- Database read replicas, and IAM or Azure token auth for the database.
- Single-underscore names for other settings, such as S3 and the secrets key.
  That's a separate request.
- Sharing the per-process in-flight cap.
- Any change to the HTTP API. Nothing here should change the OpenAPI document.

## Anti-goals

The work fails, even if it runs, if any of these happen:

- A Redis blip takes litegen down, or a dead Redis adds latency to every
  request.
- A setup that isn't safe for several replicas runs silently.
- A Redis or database password shows up in a log, an error or an API answer.
- A single replica without Redis behaves differently, apart from the changes
  listed under [Behavior without Redis](#behavior-without-redis).
- A crashed replica leaves parallel slots or poller claims held for good.
- The shared cache fills Redis.

## Configuration

### Redis

| Variable | Meaning |
|---|---|
| `LITEGEN_REDIS_URL` | The whole URL, `redis://` or `rediss://`. It wins over the parts, as in LiteLLM. **(user)** |
| `LITEGEN_REDIS_HOST` | The host. When no URL is set, a host turns Redis on. **(user)** |
| `LITEGEN_REDIS_PORT` | The port, 6379 when unset. **(user** for the variable; the default is **my call**. LiteLLM requires a port.**)** |
| `LITEGEN_REDIS_PASSWORD` | Optional. **(user)** |
| `LITEGEN_REDIS_USERNAME` | Optional, for Redis ACL users. Taken from LiteLLM. **(my call)** |
| `LITEGEN_REDIS_SSL` | `true` builds a `rediss://` URL. Taken from LiteLLM, because managed Redis such as DigitalOcean's requires TLS. **(my call)** |
| `LITEGEN_REDIS_SSL_CA_CERTS` | Optional path to a PEM file of CA certificates to trust, for managed Redis signed by a private CA. This is LiteLLM's `REDIS_SSL_CA_CERTS`. **(my call)** |
| `LITEGEN_REDIS_SSL_CERT_REQS` | `none` turns certificate checks off. The default, `required`, checks the server's certificate and host name. This is LiteLLM's `REDIS_SSL_CERT_REQS`. **(my call)** |

- With neither a URL nor a host, there's no Redis, and litegen behaves as a
  single replica does today.
- TLS checks the server's certificate by default, against the system's roots
  plus `LITEGEN_REDIS_SSL_CA_CERTS`. The two TLS settings apply to a
  `rediss://` URL as well as to the parts. Use rustls, as sqlx and rust-s3
  already do. **(my call)**
- A URL built from the parts percent-encodes the username and password.
  **(my call:** LiteLLM doesn't, so a password containing `@` breaks
  there.**)**
- If a URL and parts are both set, the URL wins, and a startup warning names
  the variables it ignored. **(my call:** LiteLLM uses the URL without a
  word.**)**
- Any Redis-protocol server works, including Valkey.

### Database (Postgres)

Every name below is accepted spelled both ways, `LITEGEN_DATABASE_*` and
`LITEGEN_DB_*`. **(user)** The parts always build a Postgres URL. **(user)**

| Suffix | Meaning |
|---|---|
| `_URL` | The whole URL: Postgres, or SQLite as today. It wins over the parts. |
| `_HOST` | The host. When no URL is set, a host turns on parts mode. |
| `_PORT` | 5432 when unset, as in LiteLLM. |
| `_USER` or `_USERNAME` | Required in parts mode. LiteLLM accepts both. |
| `_PASSWORD` | Optional. |
| `_NAME` | The database name. Required in parts mode. |
| `_SSLMODE` | Optional. Added as `?sslmode=`, as LiteLLM's `DATABASE_SSLMODE` is. It takes any mode sqlx accepts: `disable`, `prefer`, `require`, `verify-ca` or `verify-full`. As in libpq, `require` encrypts without checking the certificate, and `verify-full` checks it. DigitalOcean's managed Postgres needs at least `require`. **(my call** to include it.**)** |
| `_SSLROOTCERT` | Optional path to the CA certificate for `verify-ca` and `verify-full`, added as `sslrootcert=`. This is LiteLLM's `DATABASE_SSLROOTCERT`. **(my call)** |

The URL is `postgres://user:password@host:port/name?sslmode=…&sslrootcert=…`,
with each part percent-encoded.

Precedence, highest first:

1. `LITEGEN_DATABASE_URL` or `LITEGEN_DB_URL`;
2. `LITEGEN__DATABASE_URL`, the double-underscore name used today, which keeps
   working;
3. the parts;
4. `database_url` in `litegen.yaml`;
5. `sqlite://litegen.db`.

Edge cases, all **(my call)**:

- When a lower-precedence setting is present and ignored, a startup warning
  names both.
- If the `DATABASE` and `DB` spellings of one setting are both set and differ,
  litegen refuses to start and names both variables. If they're equal, that's
  fine.
- If parts mode is missing its user or name, litegen refuses to start and
  names the missing variable. LiteLLM skips the parts silently, which here
  would mean falling back to SQLite without a word.

`LITEGEN__DATABASE_URL` stays in use in `docker-compose.yml`,
`deploy/docker-compose.prod.yml`, CI, the Playwright configs, the Rust tests
and `scripts/backup-db.sh`. The docs switch to the new names, but those files
don't have to change.

### When Redis can't be reached

These copy LiteLLM, with the `LITEGEN_` prefix:

| Variable | Meaning |
|---|---|
| `LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT` | `true` refuses limited requests with 503 and `Retry-After` while Redis can't be reached. Default `false`. This is LiteLLM's `general_settings.fail_closed_rate_limit_enforcement`. |
| `LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD` | Default 5, as in LiteLLM. |
| `LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT` | Seconds. Default 60, as in LiteLLM. |

## Behavior with Redis set

1. **Rate limits.** The token-bucket semantics and the `X-RateLimit-*`
   headers stay as they are today. Each check is one atomic step in Redis,
   such as a Lua script, so all replicas draw from one bucket per key and per
   principal. Swapping the backend behind `RateLimiter` covers both places
   that use it.
2. **Parallel slots.** All replicas count against one cap, and the 429 is the
   same as today. A slot is a lease that expires and is refreshed while its
   request runs, so a replica that crashes frees its slots.
3. **The MCP sign-in limit** is shared per address.
4. **The provider circuit breaker** shares its failure streak and its
   open-until time per provider, and settings changes still apply live. A
   failure while it's half-open opens it again, as described under
   [Behavior without Redis](#behavior-without-redis).
   `proxy/circuit_breaker.rs` is an engine file
   ([engine boundary](../../../.claude/rules/engine-boundary.md)). So it gets
   a small engine trait for its state, and the server supplies the Redis
   implementation, as with `store::LinkSigning`. The engine never names Redis
   or any server module.
5. **The image cache** takes the two-tier shape of LiteLLM's `DualCache`:
   each replica's local cache sits in front of a shared tier in Redis.
   **(my call)**
   - The local tier stays as it is. `cache_max_items` caps it, and
     `cache_entries` and `/health` keep counting what's on this replica. So
     nothing visible changes, the OpenAPI text stays true, and `/health`
     never waits on Redis.
   - A local miss reads the shared tier, and storing an answer writes both
     tiers. Shared entries keep the answer's TTL. Answers with `b64_json` can
     be megabytes, so entries over a size cap stay local-only, and Redis
     doesn't fill up.
   - `DELETE /v1/cache` clears the shared tier, and through the reload
     message, every replica's local tier.
   - A settings change works as `GenerationCache::reconfigure` does today.
     Turning the cache off empties both tiers: the replica that saved the
     change clears the shared tier, and every replica clears its local tier
     when it applies the change. A new size empties only the local tiers,
     because the shared tier has no count cap. A new TTL applies to what's
     stored from then on.
   - If a reload message is lost, the next 15 s refresh catches up. The
     builder picks how, for example a cache epoch kept with the settings.
6. **Config sync.** An admin change to settings, routes, runtime models,
   disabled models or provider configs publishes a small "reload what
   changed" message on Redis pub/sub. Every replica then reloads that part
   from the database, which stays the source of truth. Pub/sub can drop
   messages, so the 15 s refresh stays as a backstop.
7. **Startup warnings.** When Redis is set and the database is SQLite or file
   storage is local, a warning names the setting to change. **(my call:**
   warn, don't refuse, because one replica with Redis and SQLite is a fine
   development setup.**)** The startup log names the Redis host and port
   (never the password) and says whether Redis answered.

## Behavior without Redis

A single replica keeps working as today, except for these changes. None of
them needs Redis:

- **The 15 s refresh** now also reloads runtime models, disabled models and
  provider configs (goal 3).
- **The poller claims its rows** in the database (goal 4). On Postgres that
  could be `FOR UPDATE SKIP LOCKED` or the existing `lease_generation_work`.
  Each tick's window skips rows another replica holds, and a crashed
  replica's claims lapse within a tick or two. Reaping after
  `MAX_ACTIVE_AGE_SECS` stays as it is. SQLite on a single replica keeps
  working.
- **The link-signing key.** When `LITEGEN__SECRETS_KEY` isn't set, one key is
  generated once and stored in the database, and every replica uses it (goal
  5). As a side effect, links on a single replica survive a restart.
  **(my call:** anyone who can read the database can then forge file links,
  but database access already exposes the files' metadata and URLs.**)** When
  the secrets key is set, it still wins.
- **The circuit breaker opens again.** Today `record_failure` sets
  `opened_at` only when it's empty. So once the first open window ends, a
  provider that keeps failing is never cut off again until a call succeeds.
  A failure while the breaker is half-open should open it for another
  window. Fix the local and Redis backends together, starting from a test
  that fails. **(my call:** it's a bug, found while writing this design.**)**
- **A startup notice when Redis isn't set.** It says that rate limits,
  parallel slots, the circuit breaker and the cache are per process, and
  that `LITEGEN_REDIS_URL` shares them. litegen can't count its replicas, so
  this is an info line, not a warning. **(my call)**

## Redis outage

This follows LiteLLM. **(user)**

- Every Redis call goes through one client guarded by a circuit breaker.
  After the threshold of consecutive failures, calls fail at once for the
  recovery timeout, and then one probe goes out.
- **Rate limits, parallel slots and the sign-in limit** fall back to the
  replica's own counters, with a warning in the log that doesn't flood it.
  With fail-closed on, they answer 503 with `Retry-After` instead. When Redis
  comes back, the shared counters take over again, and the local ones are
  dropped, not merged. LiteLLM's log line says the same: "counters will
  diverge from Redis until window expires".
- **The provider breaker** falls back to local state.
- **The cache** counts as a miss and never blocks a request.
- **Config sync:** the subscription reconnects, and the 15 s refresh covers
  the gap.
- **Readiness:** a Redis outage doesn't fail `/health/ready`, because that
  would pull every replica out of the balancer at once. Whether the body
  reports Redis is the builder's call. If it changes the OpenAPI schema,
  follow the frontend/SDK handshake in
  [working-in-this-repo](../../../.claude/rules/working-in-this-repo.md).

## Proof

- **Unit tests** cover how the environment resolves: every precedence step,
  both spellings, conflicts, percent-encoding, missing parts, the TLS
  settings, and the warnings for ignored settings. They also cover a breaker
  that fails its half-open probe and opens again.
- **Redis tests skip themselves** unless `LITEGEN_REDIS_TEST_URL` is set, as
  the Postgres tests do with `LITEGEN_PG_TEST_URL`. They run against a
  throwaway Redis or Valkey container.
- **The two-replica test** runs two litegen servers on one Postgres and one
  Redis, and shows each of these:
  1. a key limited to N requests per minute gets exactly N, in total, across
     both replicas;
  2. a parallel cap holds across both replicas, and a killed replica's slots
     free themselves;
  3. a model disabled on A is refused on B within about a second, and within
     15 s with pub/sub turned off. Turning it off is a switch for tests, not
     a setting users see;
  4. each active job is polled by one replica per tick;
  5. a link signed on A opens on B;
  6. with Redis stopped, requests still succeed under local limits, or get
     503 with fail-closed on, and the limits are shared again once Redis
     returns;
  7. `DELETE /v1/cache` on A clears B's local tier and the shared tier, and
     an answer cached on A is a hit on B;
  8. a provider that keeps failing stays cut off on both replicas: its
     breaker opens again after a failed half-open probe.
- **CI** gets a Redis service, with `LITEGEN_REDIS_TEST_URL` set
  (`.github/workflows/ci.yml`).

## Constraints

- **Engine boundary:** new Redis code is server code. A new top-level module
  stays server code. The engine reaches shared state only through engine
  traits ([engine-boundary.md](../../../.claude/rules/engine-boundary.md)).
- **The shared checkout**
  ([working-in-this-repo.md](../../../.claude/rules/working-in-this-repo.md)):
  - agree with the other sessions on who owns which files before editing;
  - ask before running cargo, and never run it alongside another session;
  - build with Rust 1.99.1;
  - make no commits on this laptop.
- **Adding the Redis crate:** diff `Cargo.lock` and expect only additions.
  Never use `cargo --offline`.
- **Postgres SQL changes**, such as poller claims or the stored link key: run
  the gated Postgres tests against a real container
  ([database-tests.md](../../../.claude/rules/database-tests.md)). New
  migrations go after the highest number, and applied migrations are never
  edited.
- **Docs to update:**
  - the README's scaling sections (lines 683-686 and 832-834);
  - `docs/gateway-settings.md`, its "Several replicas" section and the
    per-replica cache notes;
  - `docs/RUNBOOK.md`, line 67;
  - the Redis and database variables in the `.env` examples.

  The compose files run one replica on one host, so they get no Redis
  service. The docs show the variables for running several replicas.
  **(my call)**

## Left to the builder

- The Redis client crate and connection handling (redis-rs with its tokio
  connection manager is the obvious pick), and rustls for TLS.
- The key layout, including a `litegen:` prefix, and the Lua scripts.
- How long a slot lease lasts, and how it's refreshed.
- How the poller claims its rows.
- How the 15 s refresh notices changed models and providers cheaply.
- The cache's size cap and how entries leave Redis.
- The names of log lines and metrics. A counter of Redis fallbacks would
  help.
- The order of work. A suggested order: connection settings and startup
  warnings; the wider refresh and pub/sub; the shared limits and the outage
  behavior; the breaker and the cache; poller claims and the stored link key;
  then the two-replica test and CI.
