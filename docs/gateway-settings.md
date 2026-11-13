# Gateway settings: cache, retries, timeouts

These settings are the ones you tune while litegen runs: the response cache,
retries, the vendor timeout and the circuit breaker. Set their defaults in
`litegen.yaml`, then change them live from the dashboard (Gateway →
Settings) or the API. A live change is stored in the database, applies to the
next request, and reaches every replica within 15 seconds. This is how
LiteLLM's proxy treats its `litellm_settings` and `router_settings`.

What litegen does with stored outputs (`storage_access`: keeping, retention,
links and who may open them) is set the same way. It has its own guide:
[docs/storage-access.md](storage-access.md).

## The settings

Each one: the value stored in the database, else `litegen.yaml`'s.

| setting | `litegen.yaml` | API and database (`/v1/admin/settings`) | default | LiteLLM |
|---|---|---|---|---|
| cache image answers | `cache.enabled` | `cache_enabled` | off | `litellm_settings.cache` |
| how long an answer is kept | `cache.default_ttl_seconds` | `cache_ttl_seconds` (1–604800) | 3600 | `cache_params.ttl` |
| how many answers are kept | `cache.max_items` | `cache_max_items` (1–1000000) | 10000 | (in-memory size) |
| about how much memory they may take, in bytes, per replica | `cache.max_bytes` | `cache_max_bytes` (1048576–68719476736) | 268435456 (256 MiB) | — |
| retries after the first try | `retries.max_retries` | `retry_max_retries` (0–5) | 2 | `router_settings.num_retries` |
| longest first wait | `retries.backoff_ms` | `retry_backoff_ms` (0–60000) | 500 | `retry_after` |
| longest wait of all | `retries.max_backoff_ms` | `retry_max_backoff_ms` (≤ 60000) | 10000 | — |
| one vendor call's time limit | `server.vendor_timeout_secs` | `vendor_timeout_seconds` (5–3600) | 120 | `router_settings.timeout` |
| failures in a row that open a provider's breaker | `circuit_breaker.threshold` | `circuit_breaker_threshold` (1–1000) | 5 | `allowed_fails` |
| how long it stays open | `circuit_breaker.open_for_seconds` | `circuit_breaker_open_for_seconds` (1–3600) | 30 | `cooldown_time` |

```yaml
cache:
  enabled: true
  default_ttl_seconds: 3600
  max_items: 10000
  max_bytes: 268435456
retries:
  max_retries: 2
  backoff_ms: 500
  max_backoff_ms: 10000
server:
  vendor_timeout_secs: 120
circuit_breaker:
  threshold: 5
  open_for_seconds: 30
```

litegen refuses to start when `litegen.yaml` or a `LITEGEN__*` variable
doesn't load, for example a value outside these ranges or an unknown key under
`retries`. It no longer falls back to its defaults, which would silently drop
everything the config set.

**At runtime**, as the gateway admin:

```sh
curl -X PUT http://localhost:4000/v1/admin/settings \
  -H "Authorization: Bearer $MASTER_KEY" -H 'content-type: application/json' \
  -d '{"cache_enabled": true, "cache_ttl_seconds": 600, "retry_max_retries": 3}'
```

- **`PUT` replaces every setting:** a field you leave out goes back to
  `litegen.yaml`'s. Send the others you keep along with it; the dashboard
  does.
- **A value out of range is a 400.**
- **The answer, and `GET /v1/admin/settings`, show what is in force:**
  `effective_cache`, `effective_retries`, `effective_vendor_timeout_seconds`
  and `effective_circuit_breaker`. `cache_entries` is how many answers this
  replica's cache holds now. With Redis it counts the replica's own memory,
  not the shared tier behind it.

## The cache

The cache holds image answers, in memory, on each replica. With Redis set
(`docs/scaling.md`) it also has a shared tier behind each replica's own, so an
answer cached on one replica is a hit on the others. Video and 3D are jobs
and are never cached. An answer is keyed by the caller's app (or org)
and by every field of the request, so two tenants never share one.

A hit hands back an earlier generation's image, so it's keyed narrower for
callers who couldn't read that generation:
- under `signed` or `private` with `owner_scope: key`, by the API key too;
- for a person (a session or OAuth grant) without `generation:read:any`, such
  as a Member or Viewer, by the person.

Those callers then share fewer hits, which means more vendor calls.

**Its size.** Each replica's cache keeps at most `max_items` answers in
about `max_bytes` of memory, whichever limit is reached first; the least
used answers go first. An answer weighs about its JSON size: a link answer is a
kilobyte or two, but a `b64_json` answer carries its image, megabytes each
(with no object storage every answer is `b64_json`). At the 256 MiB default
that is about 90 answers of 2.8 MB, or many thousands of link answers;
raise `max_bytes` to keep more. The byte bound is approximate: the cache
settles its writes in batches, so a burst of large answers can be over it
for a moment. An answer larger than the whole budget is never kept, and
once the cache is full a new large answer gets in only if it has been asked
for more often than the answers it would push out.

- **A model route can decide for its own models.** A route's
  `cache_enabled` turns the cache on or off for the models it matches,
  whatever the gateway's setting. Its `cache_ttl_seconds` sets how long
  their answers are kept. Routes are set in `model_routes` in YAML, or with
  `PUT /v1/admin/routes`.
- **A request can opt out** with `Cache-Control`, as LiteLLM's `cache`
  request param does:
  - `no-cache` answers fresh, and the fresh answer is cached;
  - `no-store` never caches the answer (a cached one may still be served);
  - both together leave the cache untouched.
- **A request that keeps nothing** (`keep_outputs: false`, or
  `X-Litegen-Keep-Outputs: false`) is neither looked up nor cached: holding
  its image would be keeping it.
- **Every image answer says what happened:** `X-Litegen-Cache: hit`, `miss`
  or `bypass` (the cache is off for that model, the request said
  `no-cache`, or it keeps nothing).
- **Live changes:**
  - turning the cache off empties it, and a request that was already with
    the vendor doesn't store its answer when it comes back (nor does one
    whose route turned its cache off meanwhile);
  - turning it on empties it too, so nothing cached before (by routes with
    their own cache settings, or left by a switch-off that couldn't reach
    Redis) is served;
  - a new size starts it again, empty;
  - a new TTL applies to answers kept from then on.
- **Clearing:** `DELETE /v1/cache`, or "Clear cache" on the dashboard. With
  Redis it clears every replica: the shared tier is emptied, and the other
  replicas empty their own memory within about a second (at the latest, at
  their next 15 s refresh). Turning the cache off or on empties both tiers
  the same way. A new size resizes each replica's own cache only. When the
  shared tier can't be cleared (Redis can't be reached), `DELETE /v1/cache`
  empties this replica only and answers 503 `cache_clear_incomplete` with
  `Retry-After: 1`: retry once Redis answers. Turning the cache off or on
  still answers 200 then, since the setting reaches every replica through
  the database.
- **The shared tier:**
  - an answer keeps the time to live it was cached with;
  - an answer larger than 256 KiB (a `b64_json` image, usually) stays on the
    replica that made it;
  - if Redis can't be reached, the cache works from each replica's memory
    alone and never delays or refuses a request.
- **Counters:** `/metrics` counts lookups as
  `litegen_cache_requests_total{result="hit"|"miss"}`.

## Retries

When a vendor turns a call away with an error worth retrying (a 429, a 408, a
5xx), litegen tries again, up to `max_retries` more times. It waits before
each try:

- **The wait is random**, between 0 and the longest wait for that try: the
  first backoff, doubled each retry, never more than the longest backoff.
  This is "full jitter" (AWS Architecture Blog, "Exponential Backoff and
  Jitter"), which keeps many callers from retrying in step.
- **A vendor's `Retry-After` is a floor** (seconds or an HTTP date, on a
  429): litegen waits at least that long. If the vendor asks for longer than
  the longest backoff, litegen doesn't wait at all. The request fails with
  the vendor's answer, so your client backs off instead of litegen holding
  the request open.
- **Never retried:**
  - a call that ran out of time, since the vendor may still finish, and
    bill, the job;
  - a job the vendor already accepted;
  - a refusal of the request itself (a 400, a 401, a content filter).

  None of these is a setting.
- **A model route's deployments** keep their own `max_retries`, with the
  same waits.

## The vendor timeout and the circuit breaker

- **The vendor timeout** limits one call to a vendor, 120 s by default. A
  call that runs out of time is answered `504 vendor_timeout` and never sent
  again. A change applies to the next call.
  - Image models whose vendor works in the background (BFL, Replicate,
    Kling, Hunyuan, Leonardo, Luma and Runway image) are polled until the
    timeout, so raise it for a slow model.
  - A job the vendor is still running at the timeout is delivered later: the
    504 names the generation it goes to, and the poller finishes it
    ([async-images.md](async-images.md)).
- **The circuit breaker** stops calling a provider after `threshold`
  failures in a row, for `open_for_seconds`. Then calls go through again,
  as LiteLLM's cooldown does: the first failure after the window opens it
  for another window, and a success clears it, so the count starts again.
  A success that comes back during the window (from a request sent before
  the breaker opened) changes nothing, so a window always runs its full
  length. A new threshold counts from the next failure; a new open time
  applies to the next check, including breakers already open. Breaker state
  is per replica without Redis; with Redis set, the replicas share it.
  - **What counts as a failure:** a vendor answer of 5xx or 408, no answer
    at all, a timeout, and a 401, 402, 403 or 429 to the operator's own
    credential.
  - **What doesn't:** a refusal of one caller's request, by the vendor (any
    other 4xx, such as a moderation 422, or a verdict such as moderation
    without a status) or by litegen's adapter before any call (a combination
    the model can't make); and a 401, 402, 403 or 429 to an org's own
    credential (BYO). One tenant's bad requests, revoked key, unpaid account
    or spent rate limit never cut a provider off for everyone. On a model
    route, each deployment is judged by the credential it used.
  - A request whose route had every breaker open (or every provider
    missing) answers a retryable 502 that names why each was skipped, and
    doesn't spend its `Idempotency-Key`.

## Server limits (set at startup)

These are read from `litegen.yaml` or `LITEGEN__…` variables at startup only;
they are not gateway settings and have no database row.

| what | setting | default | range |
|---|---|---|---|
| requests that run at once | `backpressure.max_in_flight` | 64 | |
| a connection's time to send a request's headers, and the longest a keep-alive connection may sit idle | `server.header_read_timeout_secs` | 130 | 1–3600 |
| a generation, quote, segmentation or MCP request body's time to arrive, whole | `server.request_body_timeout_secs` | 300 | 1–3600 |
| what a stop (SIGTERM, ctrl-c) gives open requests and the poller's current rows | `server.shutdown_grace_secs` | the vendor timeout + 30 s (150 by default; may pass 3600 when the vendor timeout is raised) | 1–3600 |

- **A body arrives before it takes a slot.** A request's body is read first,
  each byte counted against a budget of `max_in_flight` × 25 MiB as it
  arrives; only a body that is all there takes one of the `max_in_flight`
  slots. A client sending slowly holds the bytes it sent, never a slot. Two
  readers have budgets of their own, so no body is counted twice: the hook
  layers (with hooks, a hooked request's body is counted there, not again
  behind them), and MCP (`/mcp`, `/admin/mcp`, `max_in_flight` × 16 MiB), so
  the request a tool call sends on through the API is counted once, in the
  API's budget.
- **What a client sees.** With every slot taken, or the body budget spent:
  `503 server_at_capacity` with `Retry-After: 1`. With every slot taken the
  503 comes at once, before the body is read, on the hooked routes too; only
  a request with an `Idempotency-Key` is read first, since its body is
  hashed to recognise a repeat. A body slower than its deadline:
  `408 body_timeout` (at the default, a 25 MiB body needs about 87 KB/s). A
  body over 25 MiB, declared or sent: `413 body_too_large`. A body that
  breaks off: `400 body_unreadable`. Headers slower than their deadline, or a
  connection that sends nothing: the connection is closed. One exception: a
  client speaking HTTP/2 without TLS (h2c) has no header or idle deadline
  once its connection preface is in (a stop still closes it). Behind a reverse
  proxy that talks HTTP/1.1 to litegen, as the shipped Caddyfile does, this
  does not arise.
- **Behind a reverse proxy,** keep the proxy's upstream idle timeout below
  `server.header_read_timeout_secs`, so the proxy always closes an idle
  connection first; otherwise it may send a request on a connection litegen
  is closing and answer 502. Caddy's default is 120 s, nginx's and AWS ALB's
  60 s, all below the default 130. Some proxies keep idle upstream
  connections much longer: Google Cloud's external Application Load Balancer
  600 s, and Envoy and Istio an hour by default. Behind those, raise
  `server.header_read_timeout_secs` above the proxy's timeout, or lower the
  proxy's.
- **A stop** ends within the grace: idle connections close at once, requests
  in progress and the poller's current rows get the grace, and whatever is
  still open at its end is cut ([RUNBOOK.md](RUNBOOK.md#graceful-shutdown-not-happening)).
  Give the orchestrator the grace + 15 s (Compose `stop_grace_period`,
  Kubernetes `terminationGracePeriodSeconds`), or lower the grace to fit.
- **`server.request_timeout_seconds` is retired.** It never limited anything;
  litegen now names it in a startup warning. A whole request has no limit: a
  vendor call has the vendor timeout, and headers, bodies and shutdown have
  the settings above.

## Several replicas

A change saved through the admin API reaches every replica, for all five
kinds of change: gateway settings, model routes, runtime models, disabled
models and provider configs.

- **Without Redis,** every replica reloads all five from the database every 15
  seconds and applies what differs. A change reaches the others within 15
  seconds.
- **With Redis** (see [scaling.md](scaling.md)), the replica that saved the
  change also tells the others, and they reload at once: within about a
  second. The 15-second reload still runs, so a replica that was
  disconnected, or missed the message, catches up at its next one.
- The replica that saved a change applies it at once. If the database can't
  be read, a replica keeps what it has. If Redis can't be reached, saving
  still works and the others catch up within 15 seconds.

## On the dashboard

Gateway → Settings has these sections. Each shows the values in force beside
the form, and a blank field follows `litegen.yaml`.
- **Cache:** on or off, how long answers are kept, the size, how many are
  cached now, and "Clear cache".
- **Retries and timeouts:** the number of retries, the first and longest
  backoff, and the vendor timeout.
- **Circuit breaker:** the failures that open it, and for how long.
