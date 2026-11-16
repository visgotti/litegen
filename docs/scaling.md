# Running several replicas

Several litegen replicas can serve one deployment behind a plain load
balancer, with no sticky sessions and no per-replica settings. They share the
database. Redis, when you set it, is what they share besides. Without Redis,
litegen behaves as a single replica does today, apart from the changes listed
under [Several replicas](#several-replicas).

To run several replicas, set three things:

- a **Postgres** database every replica can reach (SQLite allows one writer, on
  one machine);
- **S3** for stored files (`image_storage.backend: s3`), because local disk is
  per replica;
- a **Redis URL**, so the replicas share their limits and state.

litegen warns at startup when Redis is set and the database is SQLite or the
files are on local disk ([Startup messages](#startup-messages)).

Give every replica the same configuration, the same `LITEGEN__MASTER_KEY` and
`LITEGEN__SECRETS_KEY` included. The secrets key encrypts the provider and
storage credentials kept in the database, and signs file links when it's set.
Replicas with different keys can't read each other's stored credentials or
verify each other's links, and with different master keys they accept
different bearer tokens. Whether the provider configs' header values and
URL user info are encrypted too is one gateway setting for all replicas,
`encrypt_provider_credentials`: a version before 2026-11-15 can't read the
rows it encrypts, so turn it on once every replica runs a later version,
and off before rolling back ([RUNBOOK.md](RUNBOOK.md#provider-credentials-at-rest)).

Point the load balancer's health check at `GET /health/ready` and the
liveness probe at `GET /health/live`.
Readiness asks the database only, never a vendor, so a slow vendor can't
fail it, and a probe spends no vendor quota. `GET /health` checks the
vendors with the gateway's own keys, all at once; it is the deployment's to
read (the master key, or in single-tenant mode a `read` credential of the
deployment's org).

This page covers what the replicas share, how litegen finds its database and
its Redis, what happens when Redis is down, how to run Redis, and the startup
messages.

## Several replicas

With Redis set, the replicas share the first five things below. Without it,
each of the five is that replica's own. The rest go through the database,
so they work without Redis too, and config sync is only slower without it.

- **Rate limits.** Each limit is one token bucket in Redis: a key's
  `rpm_limit`, and the rpm of an org, app, member or end user. Taking a token
  is one atomic step that uses Redis's clock, so all replicas draw from one
  bucket, and their own clocks don't matter. The `X-RateLimit-*` headers and
  the 429 are the same as with one replica. A bucket expires 120 s after its
  last request, and an expired bucket is a full one.
- **Parallel slots.** A principal's cap on parallel requests is a set in Redis,
  one member for each request in flight. A member is a lease: it lasts 30 s
  and is refreshed every 10 s while the request runs. A replica that crashes
  refreshes nothing, so its slots free themselves within 30 s. If a member is
  gone from Redis (after a Redis restart, say), its refresher stops, and that
  request goes uncounted until it ends. A request whose answer from Redis is
  lost (a timeout, a dropped connection, a client that hung up) may still have
  taken its slot there, so litegen gives that slot back at once rather than
  leave it counted for the 30 s lease; if that fails too, the lease frees it.
  The 429 is the same as with one replica.
- **The MCP sign-in limit.** One bucket per address for `/oauth/register` and
  `/oauth/authorize` ([MCP self-hosting](mcp/self-hosting.md)).
- **The provider circuit breaker.** Each provider's failures in a row, and when
  its breaker opened, live in Redis, so all replicas stop calling a failing
  provider together. A failure while the breaker is half-open opens it again,
  with or without Redis. Changing the threshold or the open time in the gateway
  settings still applies live. A provider's state expires 24 hours after its
  last failure, and a success after its window clears it. A success during a
  window (from a request sent before the breaker opened, on any replica)
  changes nothing, so the window runs its full length on every replica. This
  is not the breaker that guards Redis itself, which is described under
  [When Redis is down](#when-redis-is-down).
- **The image cache** has two tiers: each replica's memory, and a shared tier
  in Redis behind it. A local miss reads the shared tier, and a store writes
  both. A shared entry keeps the answer's time to live. Each replica's
  memory holds about `cache.max_bytes` of answers at most (256 MiB by
  default), and no more than `cache.max_items` of them. `cache_entries` and
  `/health` count this replica's memory only. An answer over 256 KiB (a
  `b64_json` image, usually) stays on the replica that made it, so Redis
  doesn't fill up. `DELETE /v1/cache` empties the shared tier and tells the
  other replicas to empty theirs; turning the gateway cache off or on in the
  gateway settings empties both tiers on every replica the same way. See
  [the cache](gateway-settings.md#the-cache).
- **Config sync.** A saved change to gateway settings, routes, runtime models,
  disabled models or provider configs is written to the database, and the
  replica that saved it publishes a "reload" message on Redis pub/sub. The
  others read the database again within about a second. Every replica also
  reloads every 15 s, which covers a lost message, so without Redis a change
  reaches the others within 15 s. See [gateway settings](gateway-settings.md#several-replicas).
- **Poller claims.** Every 5 s, each replica's poller takes up to 100 of the
  active jobs polled longest ago (a new job counts from its creation). The
  replicas claim those rows in the database, so a vendor is asked about a job
  once a tick, not once per replica. A claim holds for 30 seconds. A replica
  that keeps polling renews its claims every tick. With more than 100 active
  jobs a replica polls them in turn, each at least once every
  `ceil(jobs / 100)` ticks, and a job it can't re-take within 30 seconds may
  pass to another replica. One that stops gracefully hands its claims back as it exits, so the others
  take those jobs at their next tick, and one that dies loses them within 30
  seconds. With 100 or fewer active jobs, one replica may poll them all, so
  more replicas add polling capacity only beyond that. A tick can run longer
  than 30 seconds (a slow vendor, a large download): while it runs, the
  replica renews the claims on the jobs it is polling every 10 seconds, so no
  other replica asks the vendor about them meanwhile. A job still waiting for
  one of the tick's 8 slots after 10 seconds is re-claimed before it is polled,
  and skipped if its claim lapsed and another replica took it. A vendor has
  30 seconds to answer a status check. One that takes longer is asked again
  at the next tick (the job isn't failed for it), so a vendor that hangs holds
  a slot for 30 seconds, not for as long as its client waits (up to 600 s).
  Downloading a finished job's files isn't counted: that has limits of its own.
  This holds for a late image too ([async-images.md](async-images.md)): its
  vendor hands the image over in the same call, and the 30 seconds stop when
  the image's download starts. A tick's jobs take turns by vendor: the first
  job of each vendor, then the second of each, and so on, each vendor's jobs
  in the order they were claimed. So the jobs of a vendor that hangs can't
  take all 8 slots while another vendor's jobs wait behind them. A finished
  3D job or video waits at most 30 seconds for one of the replica's delivery
  slots. After that the poller leaves the job in flight for the next tick. A
  3D job that keeps nothing only has its links checked, which takes no
  delivery slot. A job skipped for want of its org's key warns at its first
  skip and once at each 10-minute mark of its age, whichever replica polls
  it: the job's row records the last mark warned at (migration 035).
- **Webhook deliveries.** A job's terminal webhook is saved in the database
  with its outcome: the guarded terminal write that ends the job (the
  poller's, a reap's, a client poll's or a cancel's) records a pending
  delivery in `webhook_outbox`, in the same statement. A trigger on
  `generations` does it (migration 029, replaced by 032): it fires when a job
  moves from `pending` or `processing` to `completed`, `failed` or
  `cancelled`, has a key with a `webhook_url` (not `""`), and isn't a sync
  image result, which is where litegen sends a terminal webhook. Every
  replica runs a dispatcher. The replica whose write ended the job settles
  it, then sends the first attempt at once; the delivery is due to the other
  replicas only 30 seconds after the job ended, so none sends a webhook
  before its job's refund. Every second each dispatcher renews its claims,
  looks for a due delivery (a read, so an idle replica writes nothing), and
  only then claims the due deliveries nobody holds (up to 256 at once, 16 of
  one org), each with a lease of 60 seconds on the database's clock. Two
  replicas never send one delivery at once, and a replica that dies loses
  its deliveries to another within a minute. A failed attempt's next one is
  due at a time saved with the delivery, so a restart resumes the schedule
  where it was. A graceful stop claims no more, lets the attempts in flight
  finish within the grace, and gives back its claims on the deliveries it
  isn't sending; an attempt still sending keeps its claim until it ends or
  lapses. A delivery whose job's webhook was answered 2xx already (by a
  replica from before the outbox, during a rolling upgrade) is marked
  delivered without a request. `POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver`
  makes an ended delivery due again, and the hourly retention sweep deletes
  ended deliveries and attempts after `webhooks.log_retention_days` once that
  is set (unset, they are kept for good).
  Delivery is at least once (an attempt cut by a stop or a crash is sent
  again), so receivers drop repeats by `X-Litegen-Delivery-Id`. A database
  restored without the trigger records no deliveries: each replica then sends
  each terminal webhook from memory, as before 029, and warns `no outbox
  delivery was recorded for a terminal webhook` ([RUNBOOK](RUNBOOK.md#webhook-deliveries-failing)).
- **A deleted org's or app's files.** Deleting an org or an app records each
  file litegen kept for it in `tenant_file_deletions` (migration 031), in the
  transaction that deletes its rows. The replica that deleted it starts on
  them at once, and every minute each replica works off the due ones, 4 at a
  time, each held for 5 minutes on the database's clock: four deletes of up
  to a minute each end within that hold. Two replicas never take one file at
  once, a replica that dies leaves its files to the others once the hold has
  passed, and one that stops claims no more.
  A delete that fails is tried again after 1 minute, then 2, 4 and so on, up
  to every 6 hours, until the file is gone ([storage and access](storage-access.md#retention-and-deleting)).
- **A deleted org's or app's requests.** A request still running when its
  org or app is deleted, on any replica, removes the rows it wrote once it
  sees the tenant gone. On Postgres a request's spend tag also references its
  log row (migration 037, `ON DELETE CASCADE`), so a tag written while the
  delete runs goes with that row or is refused. During the upgrade, migration
  037 adds that key without checking the tags already there, under a brief
  lock on both tables (it waits for writes in progress, and writes wait for
  it). Migration 038 then deletes the tags whose log row is gone and checks
  the rest while writes go on.
- **Reservations.** A billed request reserves its price before it calls the
  vendor, on its key's quota and on each budget it counts against, and
  settles that to what the request cost when it ends. Each reservation is
  recorded in the database (`reservations`, migration 030) in the step that
  takes it, and the record is deleted in the step that settles it or gives it
  back. While the request runs, its replica renews the record's lease: 2
  minutes on the database's clock, renewed every 20 seconds. Every replica
  sweeps as it starts and every 20 seconds after. A record whose lease lapsed
  belongs to a request that can no longer finish, because its replica crashed
  or was killed: the sweep gives the amount back and deletes the record in one
  step, so of two replicas sweeping at once exactly one gives it back.
  Nothing is given back at a start, so a replica that restarts never frees
  what another replica's requests still hold. A replica that dies leaves its
  requests' prices held for about two minutes. A request whose replica
  couldn't renew its lease for two minutes may find its record swept when it
  settles; it is then charged its whole cost, since the sweep gave back what
  it held. A settlement that fails (the database didn't answer) is tried
  again after a few seconds, then every 30 seconds until it lands, its
  reservation still held and renewed; one that landed with its answer lost
  finds its record gone and charges nothing more. A stop cuts the retries at
  the end of its grace, and the sweep then gives that reservation back.
  On Postgres a sweep gives back its rows one at a time in a fixed order (keys
  by id, then limits rows), so two replicas' sweeps never deadlock. An
  `Idempotency-Key` in progress is held the same way: the retry of a request
  whose replica died runs once the lease lapses, instead of answering 409 for
  15 minutes, and a request still running keeps its key for as long as it
  runs. A request whose lease lapsed while it still ran, and whose key a
  retry took over, leaves the retry's record alone when it finishes. A graceful stop gives back what the requests it cuts
  hold before it exits ([RUNBOOK](RUNBOOK.md#graceful-shutdown-not-happening)).
  Parallel slots are leases in Redis already (above), and rate-limit tokens
  are never given back. On SQLite the reservation is taken by a trigger,
  `reservations_take`, as its record is inserted; litegen logs an error at
  startup if a restored database lacks it ([RUNBOOK](RUNBOOK.md#reservations-recorded-but-not-taken-sqlite)).
  Postgres takes it in one statement with no trigger.
- **Reset links follow roles.** A password reset link's issuer was checked
  against the account's roles when it was issued, so any later change to
  those roles deletes the account's unused links, and a change to the
  issuer's deletes the links it issued. Triggers do it (migrations 042 and
  043), in the transaction of the change, whichever replica or code path
  makes it:
  - on `organization_members`: a membership added (the member's links), or
    removed, or its role, org or account changed (the member's links and the
    ones it issued): joining, leaving, an owner transfer, an org's deletion;
  - on `users`: a change to the account's own `role` (the deployment's owner
    transfer included), or any write of `is_active` false, also to an account
    already inactive (its links and the ones it issued).
  Other writes to `users`, such as signing in, don't fire them. Confirming a
  link also checks its issuer again, inside its transaction, with the
  account's row locked on Postgres. Deactivating also ends the account's
  sessions, OAuth grants and unredeemed codes, in the same transaction, in
  litegen's own code. A database restored without the triggers keeps such
  links until they expire, within the hour; the confirm's check still holds.
- **Sign-in epochs.** Each account's `sign_in_epoch` (migration 043) moves on
  in the transaction of a confirmed reset, a deactivation or a change of one's
  own password. Sessions, OAuth approvals and grants record the epoch they
  began under, and the session check (in its join with the account), the code
  redemption and every use or refresh of a grant accept only a matching one.
  So a login, an approval or a code exchange that began on one replica before
  such a change and lands on any replica after it is refused. Login, invitation
  accept and social login take the epoch from the account row whose password
  hash or identity they check, and an MCP grant's use compares it in the row
  `live_member` reads. Reactivating an account moves the epoch on too. A
  password change writes only at the epoch of the session sending it. A grant
  claim made under an older epoch than the person's, or than the live grant's,
  is refused and revokes nothing; a live grant of an older epoch is revoked
  when a newer claim lands. A used code presented again revokes the live
  grant it led to only when that grant isn't newer than the code's epoch, so
  a code from before a reset can't take down the grant made after it. The
  session list shows the current epoch's sessions only. A reset link or an
  invitation is stored only while the sign-in sending it is live, in one
  statement whose check (on Postgres) takes a share lock on the sender's
  account row. A reset or a deactivation of that account, on any replica,
  either commits first, and the insert is refused (`401 sign_in_ended`), or
  waits for the insert's commit and then deletes or revokes what it stored.
- **The stored link key.** When `LITEGEN__SECRETS_KEY` isn't set, litegen
  generates one signing key for its own file links, stores it in the database,
  and every replica uses it, so a link signed on one opens on the others and
  survives a restart ([storage and access](storage-access.md)). A set secrets
  key still wins. A replica that can't read the stored key at startup tries
  again for about 8 seconds, then refuses to start rather than sign with a key
  of its own.

The database already shared the rest: budgets and key quotas, API keys (a
revoked key stops on every replica at once), login lockout, idempotency keys,
link revocation, MCP sign-in grants and each job's outcome (so its webhook is
recorded once).

The times replicas compare are the database's: poll claims, a webhook
delivery's lease, when its next attempt is due (its first-attempt grace
included) and a redeliver's cooldown, the retention sweep's cutoffs, the lease on a
finished 3D job's delivery or a finished video's keeping (taken before any of
its files are downloaded, so one replica downloads them), a 3D stage's claim, the lease of an
`Idempotency-Key` still in progress and of a reservation, and a job's age,
after which the poller fails it (2 hours). They are
stamped and judged on the database's clock, never a replica's own, so a
replica whose clock drifts doesn't take over another's delivery, fail a stage
another replica is starting, or run an idempotent request a second time.

A few things stay per replica on purpose: the cap on requests in flight in one
process (backpressure) and its budget for request bodies, the caps on webhook
attempts (16 at once for one org, 256 in all; a replica claims no more than
that, the rest wait in the database, so one org's slow receiver can't hold up
the others), the latency samples
behind lowest-latency routing, the AWS STS and JWKS caches, the cursors that
rotate through a provider's keys, and the Prometheus metrics: each replica
serves its own counters at its metrics path, so scrape every replica. That
endpoint needs no credential unless `observability.metrics_require_auth` is
set; behind the shipped `deploy/Caddyfile` it is public at `/api/metrics`
([MCP self-hosting](mcp/self-hosting.md#observability)).

## Connection settings

litegen reads its database and its Redis from environment variables. Each has
a whole-URL form and a parts form, so a managed service's host, port and
password can be set as the provider hands them out. Set the variables in the
environment, or in a `.env` file in the directory litegen starts in.

An unset variable, an empty one and one holding only spaces or line breaks are
all the same. A password is used exactly as written; every other value has its
surrounding spaces and line breaks dropped.

### Redis

| Variable | Meaning |
|---|---|
| `LITEGEN_REDIS_URL` | The whole URL, `redis://` or `rediss://`. It wins over the parts. |
| `LITEGEN_REDIS_HOST` | The host. When no URL is set, a host turns Redis on. |
| `LITEGEN_REDIS_PORT` | The port, 6379 when unset. |
| `LITEGEN_REDIS_PASSWORD` | Optional. |
| `LITEGEN_REDIS_USERNAME` | Optional, for Redis ACL users. |
| `LITEGEN_REDIS_SSL` | `true` builds a `rediss://` URL. Managed Redis such as DigitalOcean's requires TLS. |
| `LITEGEN_REDIS_SSL_CA_CERTS` | Optional path to a PEM file of CA certificates to trust, for managed Redis signed by a private CA. |
| `LITEGEN_REDIS_SSL_CERT_REQS` | `none` turns certificate checks off. The default, `required`, checks the server's certificate and host name. |

- With neither a URL nor a host, there is no Redis. If other Redis variables
  are set anyway (a password, a port, the TLS settings), a startup warning
  names them and says Redis is off.
- TLS checks the server's certificate by default, against the system's roots
  plus `LITEGEN_REDIS_SSL_CA_CERTS`. The two TLS settings apply to a
  `rediss://` URL as well as to the parts.
- A URL built from the parts percent-encodes the username and password, so a
  password containing `@`, `:` or `/` works. (In a URL you write yourself,
  encode them yourself.)
- If a URL and parts are both set, the URL wins, and a startup warning names
  the variables it ignored.
- Any Redis-protocol server works, including Valkey.

```sh
# The whole URL:
LITEGEN_REDIS_URL=rediss://default:p%40ssw0rd@redis.example.com:25061

# Or the parts (the password needs no encoding here):
LITEGEN_REDIS_HOST=redis.example.com
LITEGEN_REDIS_PORT=25061
LITEGEN_REDIS_USERNAME=default
LITEGEN_REDIS_PASSWORD=p@ssw0rd
LITEGEN_REDIS_SSL=true
```

### When Redis can't be reached

These copy LiteLLM's, with the `LITEGEN_` prefix, except
`LITEGEN_REDIS_SLOW_CALL_MS`, which is litegen's own. They apply only when
Redis is set. What litegen does while Redis can't be reached is described under
[When Redis is down](#when-redis-is-down).

| Variable | Meaning |
|---|---|
| `LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT` | `true` refuses limited requests with 503 and `Retry-After` while Redis can't be reached. Default `false`. |
| `LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD` | Failures in a row that open the Redis breaker, at least 1. Default 5. |
| `LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT` | Seconds before the breaker probes Redis again. Default 60. A recovery under 0.5 s (in whole seconds, 0) counts as 0.5 s, the longest one Redis call may take. |
| `LITEGEN_REDIS_SLOW_CALL_MS` | Milliseconds: an answer Redis takes this long or longer to give counts as a breaker failure, so a slow Redis opens the breaker as a dead one does. Default 250. `0` turns it off. A value of 500 or more has the same effect, since a call times out at 500 ms. |

### Database (Postgres)

litegen needs PostgreSQL 13 or later: the webhook outbox (migration 029) makes
its delivery ids with `gen_random_uuid()`, which is built in from 13 on. The
compose files ship 16.

Every name below is accepted spelled both ways, `LITEGEN_DATABASE_*` and
`LITEGEN_DB_*`. The parts always build a Postgres URL.

| Suffix | Meaning |
|---|---|
| `_URL` | The whole URL: Postgres, or SQLite as today. It wins over the parts. |
| `_HOST` | The host. When no URL is set, a host turns on parts mode. |
| `_PORT` | 5432 when unset. |
| `_USER` or `_USERNAME` | Required in parts mode. |
| `_PASSWORD` | Optional. |
| `_NAME` | The database name. Required in parts mode. |
| `_SSLMODE` | Optional. Added as `?sslmode=`. It takes any mode sqlx accepts: `disable`, `prefer`, `require`, `verify-ca` or `verify-full`. As in libpq, `require` encrypts without checking the certificate, and `verify-full` checks it. DigitalOcean's managed Postgres needs at least `require`. |
| `_SSLROOTCERT` | Optional path to the CA certificate for `verify-ca` and `verify-full`, added as `sslrootcert=`. |

The URL is `postgres://user:password@host:port/name?sslmode=…&sslrootcert=…`,
with each part percent-encoded.

```sh
# The whole URL:
LITEGEN_DATABASE_URL=postgres://litegen:p%40ssw0rd@db.example.com:25060/litegen?sslmode=require

# Or the parts, spelled either way:
LITEGEN_DB_HOST=db.example.com
LITEGEN_DB_PORT=25060
LITEGEN_DB_USER=litegen
LITEGEN_DB_PASSWORD=p@ssw0rd
LITEGEN_DB_NAME=litegen
LITEGEN_DB_SSLMODE=require
```

Precedence, highest first:

1. `LITEGEN_DATABASE_URL` or `LITEGEN_DB_URL`;
2. `LITEGEN__DATABASE_URL`, the double-underscore name used before, which
   keeps working;
3. the parts;
4. `database_url` in the config file (`litegen.yaml`, or the file `--config`
   names: [configuration.md](configuration.md));
5. the built-in default, `sqlite://data/litegen.db?mode=rwc`: `data/litegen.db`
   in the working directory, created with its directory
   (`/app/data/litegen.db` in the Docker image). A `litegen.db` that the old
   default (`sqlite://litegen.db`) left in the working directory is used
   instead, with a startup warning, until `data/litegen.db` exists.

Edge cases:

- When a lower-precedence setting is present and ignored, a startup warning
  names both. (A `database_url` in `litegen.yaml` is named when it differs from
  the default.) The warnings name variables, never their values.
- If the `DATABASE` and `DB` spellings of one setting are both set and differ,
  litegen refuses to start and names both variables. `LITEGEN_DATABASE_USER`
  and `LITEGEN_DATABASE_USERNAME` count as spellings of one setting too. If
  they are equal, that's fine.
- If parts mode is missing its user or name, litegen refuses to start and
  names the missing variable. It does not fall back to SQLite.
- If any database part is set with no host and no URL, litegen refuses to
  start and names the missing host variable and the parts that were set. The
  host is what turns parts mode on; without this, the parts would be dropped
  and litegen would fall back to the configured URL (SQLite by default) without
  a word. With a URL set, the URL wins and the parts are ignored.
- A host variable holds a host name or an address, nothing else. An IPv6 address
  such as `::1` is put in brackets for you. A host with a port, path or
  credentials in it (`:`, `/`, `@`, `?`, `#`), a space, or a bracket that isn't
  part of an IPv6 address refuses to start, naming the variable and the `_PORT`
  variable the port belongs in.
- A port that isn't a number from 1 to 65535, a `LITEGEN_REDIS_URL` that
  doesn't start with `redis://` or `rediss://`, and a TLS or outage setting
  with an unknown value (or a breaker threshold of 0) also refuse to start,
  naming the variable. So does a `LITEGEN_REDIS_URL` whose host and port can't
  be read: no host (such as `redis://:password`), an empty or non-numeric port,
  or a bracket that isn't part of an IPv6 address.
- A database URL whose scheme is neither `sqlite` nor `postgres` (or
  `postgresql`), such as `mysql://`, SQLAlchemy's `postgresql+psycopg://` or
  `POSTGRES://`, refuses to start, naming the scheme and nothing after it. A
  value with no scheme at all is still read as a SQLite file name. An error
  about it doesn't repeat it, since it may be a Postgres `host=… password=…`
  string, and no directory is made for it.
- A `_PORT` value that starts with `tcp://` is a Kubernetes service link, not a
  port: a Service named `litegen-db`, `litegen-database` or `litegen-redis`
  makes Kubernetes set `LITEGEN_DB_PORT`, `LITEGEN_DATABASE_PORT` or
  `LITEGEN_REDIS_PORT` that way in every pod. litegen ignores it and logs a
  warning; `enableServiceLinks: false` on the pod, or another name for the
  Service, stops it.

The older `LITEGEN__DATABASE_URL` stays in use in `docker-compose.yml`,
`deploy/docker-compose.prod.yml` and the test setups. Prefer the new names in
new deployments.

#### Statement and lock timeouts

On Postgres, litegen limits each statement on the connections of its pool:

- a statement runs for at most **5 minutes** (`statement_timeout`);
- it waits for a lock for at most **30 seconds** (`lock_timeout`).

A statement past either fails, as on any database error, rather than hold a
connection (and in the poller, a slot) for as long as it runs or another
transaction holds its rows. The poller asks about that job again at its next
tick. The migrations at startup run first, on a connection of their own
without these limits, so a long one is never cut.

A limit you set yourself wins. litegen sets each one only on a connection
where nothing else has: the URL's `options`
(`?options=-c%20statement_timeout%3D15min`), `PGOPTIONS`, or a setting of the
role, the database or the server (`ALTER ROLE litegen SET lock_timeout =
'1min'`). A `0` set this way turns the limit off. Deleting an org or an app
is the one exception to the statement limit: its transaction runs without
one (`SET LOCAL statement_timeout = 0`), so an org or an app with millions of
logs or jobs is deleted all the same. Its lock limit stays.

## When Redis is down

A Redis that is down or slow doesn't take litegen down. It costs a request at
most 500 ms per Redis call, and only until the Redis breaker below opens.
litegen starts even when Redis is down, and keeps trying. (Settings that can
never work, such as a CA file that can't be read, do stop litegen at startup.)
The settings for this are under
[When Redis can't be reached](#when-redis-cant-be-reached).

- **One guarded client.** Every Redis call goes through one client. A call,
  connecting included, takes at most 500 ms. After
  `LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD` failures in a row (5), the
  Redis breaker opens, and calls fail at once without touching the network. So
  a dead Redis adds no latency once the breaker is open. A timeout or a
  connection error counts as a failure, and so does a reply saying the node
  can't serve: `READONLY` (a primary demoted by a failover; for a write
  inside a script, Redis 5 and 6 wrap it in an `ERR Error running script`
  reply, which counts the same), `LOADING` (a Redis reading its data after a
  restart) or `MASTERDOWN` (a replica cut off from its primary). Any of these
  drops the connection, so the next call connects afresh: behind a DNS name
  that moves on a failover, that reaches the new primary. The calls in flight
  on a connection that is lost count as one failure, not one each. Any other
  error reply (another script error, `WRONGTYPE`, `OOM`) doesn't count,
  because Redis is there. A Redis still loading its data therefore opens the
  breaker after five calls, and the
  shared counters come back at the first probe after loading ends, up to the
  recovery timeout later. An answer that Redis takes
  `LITEGEN_REDIS_SLOW_CALL_MS` (250 ms) or longer to give counts as a failure
  too, though the caller still gets it: a Redis that answers just inside the
  500 ms would otherwise add that to every request without ever opening the
  breaker. One stall counts once: a slow answer counts only if its call went
  out after the last slow answer that counted, so a single hiccup that delays
  every call in flight doesn't open the breaker. Only the time Redis takes to
  answer counts, not connecting. After
  `LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT` seconds (60, and never less
  than 0.5 s), one call goes out as a probe. If it succeeds, the breaker
  closes. If it fails, the breaker opens for another recovery period.
- **Rate limits, parallel slots and the sign-in limit fall back** to the
  replica's own counters. litegen logs a warning that names the error (never
  the URL), at most once a minute for each of them. While it lasts, a limit
  applies per replica, so N replicas allow up to N times it. A take whose
  answer was lost (a timeout) may still have counted in Redis; the request
  then counts against the shared minute and the local one, as it really
  happened. Under fail-closed, a request refused that way may still have
  counted against the minute: one token too many, refilled within the minute.
- **Fail-closed refuses instead.** With
  `LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT=true`, a request that has one of
  those limits to check, and can't have it checked against Redis, gets a 503
  with `Retry-After: 1`. Nothing is counted locally. Requests with no limit to
  check aren't affected. The 503s go on until the Redis breaker closes, which
  can be up to the recovery timeout after Redis is back (see the next bullet).
  A Redis that keeps answering slowly (250 ms or more a call) opens the
  breaker too, so under fail-closed it gives these 503s rather than slow
  answers; set `LITEGEN_REDIS_SLOW_CALL_MS=0` to keep waiting for it instead.
  With Redis full under `maxmemory-policy noeviction`, the writing scripts get
  error replies, which leave the breaker closed (Redis answered), so every
  limited request and MCP sign-in gets this 503 until memory is freed, with
  one warning a minute.

  ```json
  {"error": {
    "message": "Rate limit enforcement unavailable: request counters could not be verified against Redis, and LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT is on, so the request was refused to avoid exceeding the configured limit. Retry shortly.",
    "type": "rate_limit_unverifiable",
    "code": "rate_limit_unverifiable"
  }}
  ```

- **When Redis is back,** the shared counters take over when the Redis breaker
  closes. That happens at the first probe that succeeds, and a probe goes out
  up to `LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT` seconds (60 by
  default) after Redis is back. A lower value shortens the wait. Until then the
  fallback above stays in force. The local counters are dropped, not merged: a
  minute after the last fallback, when every local rate-limit bucket has
  refilled anyway, they are cleared. (Dropping sooner would let a Redis that
  flaps hand out full limits at each blip.)
- **The provider circuit breaker** falls back to this replica's own record of
  the failures it saw.
- **The cache** never refuses a request, and Redis can hold one up for at most
  the same 500 ms per call, until the breaker opens. A lookup that can't reach
  Redis is a miss, and a store that can't stays on this replica.
  `DELETE /v1/cache` while Redis is down (or slow enough that the Redis
  breaker opens part way: each page of the clear is a call of its own)
  clears this replica only, logs a warning, and answers 503
  `cache_clear_incomplete` with `Retry-After: 1`; the dashboard's Clear
  buttons show that error. The other replicas keep their answers until those
  expire, or until a clear that finishes. Retry once Redis answers. Turning
  the cache off or on still saves and applies while Redis is down; the
  shared tier is then emptied by the next switch or clear that reaches it.
- **Config sync** reconnects, waiting 1 s at first and up to 30 s between
  tries, and reloads from the database once after each reconnect. Saving a
  change still works, and the other replicas get it within 15 s.
- **Readiness.** A Redis outage doesn't fail `/health/ready`. That would pull
  every replica out of the balancer at once.

## Running Redis

- **Version.** Redis 5 or later, or Valkey. litegen's scripts read Redis's
  clock (`TIME`) and then write, which Redis 5 and later allow because they
  replicate a script's effects, not its text.
- **A Redis of its own, or a database index of its own.** Every litegen key
  starts with `litegen:`, but the memory limit and its eviction policy (below)
  apply to the whole Redis, and `FLUSHDB` empties a whole database. A database
  index goes in the URL: `redis://host:6379/2`. (The parts have no place for
  one.) Pub/sub channels ignore the database index, though, so two
  deployments that share one Redis on different indexes see each other's
  reload and cache-clear messages. That is harmless. A reload message makes a
  replica refresh from its database once more than it needed to. A cache-clear
  message costs one read of the replica's own cache epoch, which lives in its
  own database index, so it finds nothing changed and clears nothing. A Redis
  for each deployment avoids both. Redis Cluster and Sentinel aren't
  supported: litegen talks to one address.
- **Keep it private.** Redis holds cached answers, which may carry signed URLs.
  When litegen and Redis run on different hosts, Redis needs a password or an
  ACL user, TLS (a `rediss://` URL or `LITEGEN_REDIS_SSL=true`), and a private
  network.
- **`maxmemory-policy volatile-lru`, and a `maxmemory`.** Every litegen key
  except the cache epoch has a TTL: rate buckets, slot sets, provider-breaker
  state and cached answers. With `volatile-lru` (or another `volatile-*`
  policy), a full Redis drops expiring keys and keeps the epoch. With the
  default `noeviction` it refuses writes instead. The shared cache has no cap
  on how many answers it holds, only a TTL and 256 KiB for each, so set a
  `maxmemory`.

  This one is for local use: it listens on the loopback address only, and has
  no password.

  ```sh
  docker run -d --name litegen-redis -p 127.0.0.1:6379:6379 redis:7-alpine \
    redis-server --maxmemory 256mb --maxmemory-policy volatile-lru
  ```

- **Persistence is optional.** Buckets, slots and breaker state are short-lived,
  and a bucket that is gone is a full one. One thing does care: the cache
  epoch counts the clears of the shared cache. If Redis loses its data, the
  epoch restarts at 0, and a clear in the 15 s after that can match an epoch a
  replica already recorded, so that replica doesn't drop its own copies of the
  answers. Persistence (AOF or RDB) avoids this, and so does restarting
  litegen.
- **Connections.** Each replica holds two: one for commands and one for pub/sub.

## Startup messages

litegen says at startup what it found. None of these shows a password, and a
setting is named by its variable, never its value.

- **Redis is off.** An info line: "Redis is not set: rate limits, parallel
  slots, the circuit breaker and the cache are per process.
  `LITEGEN_REDIS_URL` shares them across replicas." litegen can't count its
  replicas, so a lone replica sees this line too. If Redis variables are set
  without a URL or a host, a warning before it names them and says Redis is
  off.
- **Redis is set, and something defeats sharing.** Warnings, not refusals,
  because one replica with Redis and SQLite is a fine development setup:
  - the database is SQLite, which only one replica can use: set
    `LITEGEN_DATABASE_URL` (or the `LITEGEN_DATABASE_*` parts) to a Postgres
    database;
  - stored files are on local disk, where the other replicas can't read them:
    set `image_storage.backend` to `s3` (`LITEGEN__IMAGE_STORAGE__BACKEND=s3`),
    with `LITEGEN_S3_BUCKET`, `LITEGEN_S3_REGION`, `LITEGEN_S3_ACCESS_KEY_ID`
    and `LITEGEN_S3_SECRET_ACCESS_KEY`, and `LITEGEN_S3_ENDPOINT_URL` for an
    S3-compatible service. Without the two keys (a blank one counts as
    unset), the S3 client uses the AWS credential chain (`AWS_ACCESS_KEY_ID`,
    `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN`, a profile, web identity,
    or a container or instance role); without a bucket, or with a key but no
    secret, litegen refuses to start.
- **Whether Redis answered.** `Redis at <host:port>: reachable`, or a warning,
  `Redis at <host:port>: unreachable (<reason>); litegen keeps running and
  retries`. The host and port come from the URL, with its credentials cut off.
- **Settings that lost.** A warning names a variable that was ignored (a URL
  over its parts, a database part with a URL set, a Kubernetes service link in
  a `_PORT` variable) and the one that won.
