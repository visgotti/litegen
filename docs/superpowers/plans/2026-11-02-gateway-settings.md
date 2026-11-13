# Gateway settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The cache, retries, vendor timeout and circuit breaker become gateway settings, held in the database over `litegen.yaml` and edited on the dashboard. Per-route cache settings take effect, retries back off and honour `Retry-After`, and every replica picks up a change within 15 s.

**Architecture:**
- One `EffectiveSettings` is resolved from `AppConfig` plus `GatewaySettings`. It is recomputed whenever the overrides change (`set_overrides`), and the cache, the retry loops, the vendor timeout and the breaker read from it.
- The cache gains per-entry expiry (moka `Expiry`) and request opt-outs.
- A shared `retry_delay()` drives every submit loop.
- A 15 s refresher reloads the overrides from the database.

**Tech Stack:** Rust (axum, moka 0.12 `Expiry`, metrics), the TS and Python SDKs (regenerated), the React dashboard, Playwright.

**Spec:** `docs/superpowers/specs/2026-11-02-gateway-settings-design.md`

## Global Constraints

- A deployment that sets nothing behaves as today, except that retries now wait (full jitter, base 500 ms, ceiling 10 s).
- Ranges:
  - TTL 1–604800;
  - size 1–1,000,000;
  - retries 0–5;
  - backoffs 0–60000 ms, with the longest ≥ the first;
  - vendor timeout 5–3600 s;
  - threshold 1–1000;
  - seconds open 1–3600.
- `may_resend` decides **whether** to retry, unchanged; this work decides only **when**.
- Shared checkout:
  - no commits without the user's go;
  - `CARGO_INCREMENTAL=0`, one cargo at a time;
  - ping the peer before editing `openapi.rs`;
  - `src/mesh/` untouched.
- No subagents.

## Review Focus

1. **A `Retry-After` longer than the ceiling** must fail the request rather than hold it open. Tested in Task 3.
2. **A cache hit served to a request that opted out** (`no-cache`/`no-store`), or one stored for a `no-store` request. Tested in Task 2.
3. **A route that turns the cache off** while the global switch is on (and the reverse). Tested in Task 2.
4. **A settings refresh that fails** (database down) must keep the current settings. Tested in Task 5.
5. **Resizing the cache while requests run** must not panic or serve a stale tenant's entry. The swap is atomic behind a lock; tested in Task 2.

---

### Task 1: The settings model

**Files:**
- Modify:
  - `src/config/mod.rs`: a new `retries: RetriesConfig { max_retries, backoff_ms, max_backoff_ms }`, `deny_unknown_fields`, validated; checks on `cache`, `server.vendor_timeout_secs` and `circuit_breaker` ranges at load. A bad value stops startup, as `storage_access` does, by generalising `storage_access_in` to `guarded_sections_in(dir, env) -> Vec<&str>` covering `storage_access`, `cache`, `retries`, `circuit_breaker` and `server.vendor_timeout_secs`.
  - `src/management/types.rs`: the `GatewaySettings` fields from the spec, and view fields `effective_cache`, `effective_retries`, `effective_vendor_timeout_seconds`, `effective_circuit_breaker`, `cache_entries`.
  - `src/management/runtime.rs`: validation.
  - Create `src/management/settings.rs`: `EffectiveSettings { cache: CacheSettings { enabled, ttl, max_items }, retries: RetrySettings { max_retries, backoff, max_backoff }, vendor_timeout, breaker: BreakerSettings { threshold, open_for } }` and `fn effective(config: &AppConfig, gateway: &GatewaySettings) -> EffectiveSettings`.
  - `src/proxy/router.rs`: `ProxyRouter::settings() -> EffectiveSettings`, recomputed in `set_overrides` and `new`.
  - `src/api/handlers/management/admin.rs`: the view.

**Tests:**
- `retries_default_to_two_with_half_a_second_backoff_and_refuse_bad_values` (config);
- `gateway_settings_validation` (extended);
- `effective_settings_layer_the_gateway_over_the_yaml` (settings.rs);
- `a_bad_cache_or_retry_value_stops_startup` (hooks_e2e, binary).

Steps: tests → fail → implement → pass.

### Task 2: The cache

**Files:**
- Modify:
  - `src/proxy/cache.rs`:
    - per-entry expiry: `put_image(…, ttl: Duration)`;
    - `reconfigure(&self, CacheSettings)`, which disables and clears, rebuilds on a size change, or keeps the cache;
    - hit and miss counters;
    - interior mutability through `RwLock<Option<Cache>>`.
  - `src/proxy/router.rs` `generate_image`:
    - a route's `cache` decides enabled and TTL;
    - a `CachePolicy { lookup: bool, store: bool }` comes from the request's `Cache-Control`, passed in from the handler;
    - the answer reports `CacheOutcome { Hit, Miss, Bypass }`.
  - `src/api/handlers/mod.rs` `generate_image`: read `Cache-Control`; set `X-Litegen-Cache`.
  - Wherever the cache is constructed (`GenerationCache::new(&CacheGlobalConfig)` call sites): unchanged, with `reconfigure` called from `set_overrides`.

**Tests** (`tests/gateway_settings_api.rs`, new):
- `the_cache_header_says_hit_miss_or_bypass`;
- `cache_control_no_cache_and_no_store_opt_out`;
- `a_route_turns_the_cache_on_or_off_for_its_models` (runtime route through `PUT /v1/admin/routes`);
- `a_routes_ttl_expires_its_entries` (TTL 1 s);
- `turning_the_cache_off_live_empties_it_and_resizing_rebuilds_it`;
- `cache_counters_reach_metrics`.

Steps: tests → fail → implement → pass.

### Task 3: Retries with backoff

**Files:**
- Modify:
  - `src/proxy/router.rs`:
    - `fn retry_delay(attempt: u32, err: &ProviderError, s: &RetrySettings, rng) -> Option<Duration>`; `None` means don't retry (a `Retry-After` over the ceiling);
    - every submit loop: `0..=s.max_retries`, or the deployment's own `max_retries`, sleeping `retry_delay` before each retry.
  - `src/providers/mod.rs`:
    - `RateLimited` gains `message` and `provider_error` (it is constructed nowhere today);
    - `read_vendor_json` answers `RateLimited { retry_after_ms }` for a 429 with `Retry-After` (seconds or an HTTP date);
    - `VendorError::from_provider` keeps the body.
  - `src/providers/image/mock.rs` and `models/mock.yaml`: a new `mock/rate-limited-image`. Its first call per prompt answers `RateLimited` with the delay the prompt names (`wait=300`); later calls succeed. Plus `rate_limited_calls(prompt)`.

**Tests:**
- unit, in `router.rs`:
  - `the_backoff_grows_with_full_jitter_up_to_the_ceiling`;
  - `retry_after_is_a_floor_and_past_the_ceiling_ends_the_retries`;
- unit, in `providers/mod.rs`: `a_429_with_retry_after_is_rate_limited_in_seconds_or_a_date`;
- API:
  - `a_rate_limited_call_is_retried_after_its_retry_after`: 2 calls, elapsed ≥ 300 ms;
  - `no_retries_allowed_means_the_first_refusal_stands`;
  - `a_retry_after_past_the_ceiling_is_not_waited_for`.

### Task 4: Vendor timeout and breaker, live

**Files:**
- Modify:
  - `src/proxy/router.rs`: every `self.config.server.vendor_timeout()` becomes `self.settings().vendor_timeout`;
  - `src/proxy/circuit_breaker.rs`: `threshold`/`open_for` become atomics, plus `reconfigure(threshold, open_for)`, called from `set_overrides`.

**Tests:**
- `the_vendor_timeout_changes_live`: `mock/slow-image` with a 5 s timeout set at runtime answers 504 in under 10 s;
- `the_breaker_threshold_changes_live` (unit on `CircuitBreaker`, plus an API check through the settings view).

### Task 5: Every replica in step

**Files:**
- Modify:
  - `src/management/runtime.rs`: `pub async fn refresh(state) -> Result<bool, sqlx::Error>`, which reloads the gateway settings and routes and applies them when they differ;
  - `spawn_settings_refresher(state, every)`;
  - `src/main.rs`: spawn it every 15 s.

**Tests:**
- `a_setting_saved_on_one_instance_reaches_another_after_a_refresh`: two apps over one sqlite file;
- `a_failed_refresh_keeps_the_settings` (unit, with a store whose read fails).

### Task 6: API, SDKs, dashboard, docs

**Files:**
- `openapi.rs` (ping the peer first): `GatewaySettings`/`GatewaySettingsView` fields, plus `EffectiveCache`, `EffectiveRetries`, `EffectiveBreaker` schemas. Regenerate the SDKs; Python and TS tests stay green.
- `dashboard/src/pages/Gateway.tsx`: the Cache, Retries and timeouts, and Circuit breaker sections. The form-to-body conversion and validation are pure functions in `components/gateway-settings.ts`, with a vitest.
- `dashboard/e2e-access/gateway-settings.spec.ts`: save, a bad value refused, cache clear.
- `docs/gateway-settings.md` (new), with each setting's LiteLLM name.
- The README, `litegen.example.yaml` (`retries:`), `docs/management-api.md` (the settings row) and `known-gaps.md` (the retries gap closed).

**Final:** self-review, the full gate (Postgres), the dashboard suites, the ledger. No commit.
