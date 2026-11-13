# Gateway settings in the database: cache, retries, timeouts — design

Status: the second half of the operator's 2026-11-02 request ("cache/config
settings that can be set in the proxy settings db or config like litellm
does"), continued on their "continue". The design calls are mine. They follow
the first half (`2026-11-02-output-access-design.md`), whose layering they
reuse.

## Intent

LiteLLM keeps proxy settings in `config.yaml` and lets operators change the
ones they tune live from its admin UI, stored in its database:
- the cache (`litellm_settings.cache`, `cache_params`);
- retries, timeouts and cooldowns (`router_settings.num_retries`,
  `retry_after`, `timeout`, `allowed_fails`, `cooldown_time`).

litegen already keeps a few gateway settings in its database (markup, alerts,
end-user budget defaults, stored-file access), layered over `litegen.yaml`
and edited on the dashboard's Gateway page. This extends that to the cache,
retries, the vendor timeout and the circuit breaker. It also fixes what
stands in the way of using them:

- **Per-route cache settings are ignored today.** `model_routes[].cache`, and
  a runtime route's `cache_enabled` and `cache_ttl_seconds`, are parsed and
  never read; only the global switch decides.
- **Retries have no backoff** (known gap): a retryable submit error is sent
  again at once, `Retry-After` is ignored, and `RateLimited` is never
  constructed.
- **Settings saved on one replica reach no other** until it restarts: they
  are read at boot only.

## Decisions

### 1. The settings

| setting | `litegen.yaml` | gateway setting (database) | default |
|---|---|---|---|
| cache on | `cache.enabled` | `cache_enabled` | off |
| cache TTL | `cache.default_ttl_seconds` | `cache_ttl_seconds` | 3600 |
| cache size | `cache.max_items` | `cache_max_items` | 10000 |
| retries after the first try | `retries.max_retries` (new) | `retry_max_retries` | 2 |
| first backoff | `retries.backoff_ms` (new) | `retry_backoff_ms` | 500 |
| longest backoff | `retries.max_backoff_ms` (new) | `retry_max_backoff_ms` | 10000 |
| vendor call timeout | `server.vendor_timeout_secs` | `vendor_timeout_seconds` | 120 |
| breaker: failures to open | `circuit_breaker.threshold` | `circuit_breaker_threshold` | 5 (as today) |
| breaker: seconds open | `circuit_breaker.open_for_seconds` | `circuit_breaker_open_for_seconds` | 30 (as today) |

- **Layering:** each field is resolved on its own, the gateway setting else
  the YAML. For the cache, a model route's own `cache` comes first for that
  model.
- **`GET /v1/admin/settings`** shows the values in effect:
  `effective_cache`, `effective_retries`, `effective_vendor_timeout_seconds`
  and `effective_circuit_breaker`. It also shows `cache_entries`, the cache's
  current size.
- **Validation:**
  - TTL 1–604800 s;
  - size 1–1,000,000;
  - retries 0–5;
  - backoffs 0–60,000 ms, with the longest ≥ the first;
  - vendor timeout 5–3600 s;
  - threshold 1–1000;
  - seconds open 1–3600.

  A bad gateway value is a 400. A bad YAML value stops startup, as
  `storage_access` does, since a defaults fallback would silently change
  behaviour.
- **YAML defaults are unchanged.** A deployment that sets nothing behaves as
  today, with one exception: retries now wait between tries (decision 3).

### 2. The cache

- **Each entry carries its own lifetime:** its route's TTL, else the
  gateway's. moka's `Expiry` makes a TTL change apply to new entries without
  a rebuild.
- **A route's `cache.enabled` decides for its models**, true or false, over
  the global switch. A route with no `cache` follows the global switch.
- **A request can opt out** with `Cache-Control` (as HTTP caches read it;
  LiteLLM's equivalent is its `cache` request param):
  - `no-cache` skips the lookup but stores the fresh answer;
  - `no-store` keeps nothing (a lookup may still hit);
  - both together leave the cache untouched. These are LiteLLM's
    semantics: the two are independent.
- **Every image answer says what the cache did:** `X-Litegen-Cache: hit`,
  `miss` or `bypass`. `bypass` means the cache is off for it, or the request
  opted out of the lookup.
- **Live changes:**
  - turning the cache off empties it;
  - changing its size rebuilds it, empty;
  - a TTL change applies to entries stored after it.
- **Counters:** hits and misses go to `/metrics` as
  `litegen_cache_requests_total{result}`.
- **The cache stays as it is otherwise:** images only (video and 3D are async
  jobs), in-process per replica, keyed by tenant and the full request.

### 3. Retries

- **When to retry is unchanged.** It is `may_resend`: a retryable error, never
  after a timeout, never once the vendor accepted the job. The number of
  retries comes from the setting; a route deployment's own `max_retries`
  still wins on its route.
- **Waits:**
  - Before retry *n*, litegen waits a random time between 0 and
    `min(max_backoff, backoff · 2^(n−1))`. This is "full jitter" (AWS
    Architecture Blog, "Exponential Backoff and Jitter").
  - A vendor's `Retry-After` is a floor. If it asks for longer than
    `max_backoff`, litegen doesn't wait: the request fails with the vendor's
    answer, so the caller backs off rather than litegen holding a request
    open.
- **`read_vendor_json` reads `Retry-After`** (seconds, or an HTTP date) on a
  429. It answers `ProviderError::RateLimited`, which keeps the vendor's
  message and body, so every adapter that uses it gets the floor.
- **The wait counts against nothing billed:** a refused submit created no
  job.
- **It covers every synchronous submit loop:** image generation, segmentation,
  video and 3D submits, and routed deployments.

### 4. Vendor timeout and circuit breaker, live

- **The vendor timeout** is read from the effective settings at each call.
- **The circuit breaker** keeps its per-provider state. A change of threshold
  or seconds open applies to the next check.

### 5. Every replica in step

Every 15 s, each replica reloads the gateway settings and runtime routes from
the database and applies what changed. The replica that saved a change
applies it at once. A database error keeps the current settings and logs.
Runtime models and provider configs keep their current behaviour, loaded at
boot and on their own API calls; reloading those is a follow-up.

### 6. Where it shows

- **The dashboard's Gateway → Settings tab** gains three sections:
  - "Cache": on, TTL, size, the entry count, and a "Clear cache" button
    calling `DELETE /v1/cache`;
  - "Retries and timeouts";
  - "Circuit breaker".

  Each section shows the values in effect.
- **Docs:** `docs/gateway-settings.md`, a new guide covering every setting
  with its YAML, dashboard and API forms and the LiteLLM name it
  corresponds to. It is linked from the README and from
  `docs/management-api.md`.

## Non-goals

- A shared (Redis) cache or shared breaker state across replicas.
- Caching video and 3D answers; semantic caching; per-key cache rules.
- Reloading runtime models and provider configs on a timer.

## Anti-goals

- **A deployment that sets nothing behaves differently**, other than retries
  now waiting.
- **Retrying anything `may_resend` refuses today:** a timeout, or an accepted
  job.
- **A cache hit across tenants**, or one that ignores a request's opt-out.

## Constraints

- No commits without the user's go.
- `CARGO_INCREMENTAL=0`, one cargo at a time.
- Ping the peer session before editing `openapi.rs`.
- `src/mesh/` is untouched.
- No subagents.

## Proof

- **Unit:**
  - the backoff bounds (jitter, cap, `Retry-After` floor and ceiling);
  - `Retry-After` parsing (seconds and HTTP date);
  - settings layering and validation;
  - cache per-entry TTL.
- **API (real router, mock provider):**
  - a route's cache off and on, and its TTL;
  - `Cache-Control: no-cache` and `no-store`;
  - the `X-Litegen-Cache` header;
  - turning the cache off and resizing it live;
  - a rate-limited mock: it answers after one retry that waited at least its
    `Retry-After`; with no retries allowed it fails; with a `Retry-After`
    over the ceiling it isn't retried;
  - the vendor timeout and breaker threshold changed live;
  - a setting saved on one app instance reaches another over the same
    database after a refresh.
- **Browser:** the Gateway settings sections save, refuse a bad value, and the
  cache's "Clear" empties it.
- **The full gate** (Postgres included) and the dashboard suites stay green.
