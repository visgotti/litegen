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
different bearer tokens.

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
each of the five is that replica's own. The last three go through the
database, so they work without Redis too, and config sync is only slower
without it.

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
  and skipped if its claim lapsed and another replica took it.
- **The stored link key.** When `LITEGEN__SECRETS_KEY` isn't set, litegen
  generates one signing key for its own file links, stores it in the database,
  and every replica uses it, so a link signed on one opens on the others and
  survives a restart ([storage and access](storage-access.md)). A set secrets
  key still wins. A replica that can't read the stored key at startup tries
  again for about 8 seconds, then refuses to start rather than sign with a key
  of its own.

The database already shared the rest: budgets and key quotas, API keys (a
revoked key stops on every replica at once), login lockout, idempotency keys,
link revocation, MCP sign-in grants and each job's outcome (so its webhook goes
out once).

The times replicas compare are the database's: poll claims, the lease on a
finished 3D job's delivery or a finished video's keeping (taken before any of
its files are downloaded, so one replica downloads them), a 3D stage's claim, an `Idempotency-Key` still in
progress, and a job's age, after which the poller fails it (2 hours). They are
stamped and judged on the database's clock, never a replica's own, so a
replica whose clock drifts doesn't take over another's delivery, fail a stage
another replica is starting, or run an idempotent request a second time.

A few things stay per replica on purpose: the cap on requests in flight in one
process (backpressure) and its budget for request bodies, the caps on webhook
deliveries (16 at once for one org, 256 in all; the rest wait their turn in
memory, so one org's slow receiver can't hold up the others), the latency samples
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
