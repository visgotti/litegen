# Provider Routing Strategies & Configurable Failover — Design

**Date:** 2026-06-24
**Status:** Approved (design); pending spec review
**Area:** litegen-core providers, org provider credentials API, dashboard, SDKs

## Summary

Org-scoped BYO provider credentials already support **multiple API keys per provider with
per-key weights**, and the backend distributes traffic across them via **weighted
round-robin** ([`ApiKeyPool`](../../../litegen-core/src/providers/mod.rs) / `CredentialPool`).
The weight is the small numeric field (default `1`) in the dashboard credential editor.

What's missing: the routing behavior is **hardwired**. There is no way to:

1. Choose a **routing strategy** other than weighted round-robin.
2. Configure what happens when a key **fails** (429 / auth / quota) — today a failed call
   just returns the error to the caller; there is no key-level retry (only deployment-level
   fallback in `proxy/router.rs`).

This feature makes both **selectable per provider, per org**, stored as **plaintext**
config (keys stay encrypted), surfaced and editable in the dashboard **without re-entering
keys**, and persisted through both SDKs (TypeScript + Python).

## Goals

- Add selectable routing strategies per `(org, provider)`:
  - `weighted_round_robin` (existing default)
  - `weighted_random` (litellm "simple-shuffle"): weighted random pick, stateless
  - `least_busy`: route to the key with the fewest in-flight requests
- Add configurable failure handling per `(org, provider)`:
  - `none` (default — current behavior; error returned to caller)
  - `retry_no_cooldown`: on failure, retry the request on the next key (excluding already
    tried), up to the number of keys
  - `retry_cooldown`: same retry, plus cool down the failed key for `cooldown_seconds`;
    cooled keys are skipped during selection until the window expires
- Store routing config in **plaintext**, separate from the encrypted key blob.
- Surface routing config in the credentials list response so the dashboard can render and
  edit it **without re-entering keys**.
- Persist the new typings through the TS and Python SDKs (regenerated from utoipa OpenAPI).

## Non-Goals

- Latency-based / usage-based (TPM-RPM) routing — future follow-up.
- Cross-*provider* routing changes — this is strictly about selecting among the multiple
  keys of a single provider. Deployment-level fallback in `proxy/router.rs` is untouched.
- Persisting cooldown / in-flight state across process restarts or across nodes. All
  runtime selection state (cursor, in-flight counts, cooldown timers) is in-process, keyed
  by credential fingerprint, exactly like the existing rotation cursor.

## Background — current state (confirmed)

- **Table** `org_provider_credentials`
  ([migration](../../../litegen-core/migrations/postgres/20240101000013_org_provider_credentials.sql)):
  `id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at`,
  `UNIQUE(org_id, provider)`. The `ciphertext` is an AES-256-GCM blob of the credentials
  JSON, e.g. `{ "api_keys": [{ "key", "weight", "label" }], ... }` (or `credential_sets`
  for SigV4/TC3 signing providers).
- **Save handler** `create_org_provider_credential`
  ([orgs.rs](../../../litegen-core/src/api/handlers/orgs.rs)) takes
  `CreateProviderCredentialRequest { provider, credentials: serde_json::Value }`, derives a
  `display_hint`, encrypts the whole credentials JSON, and calls
  `db.upsert_org_provider_credential(...)` — **upsert**, keyed by `(org_id, provider)`.
- **List handler** `list_org_provider_credentials` returns
  `Vec<ProviderCredentialInfo { provider, display_hint, created_at }>` — never plaintext.
- **Selection**: `ApiKeyPool` / `CredentialPool`
  ([providers/mod.rs](../../../litegen-core/src/providers/mod.rs)) build a weighted schedule
  and rotate via a **process-global atomic cursor keyed by credential fingerprint** (so
  rotation survives the per-request pool rebuild).
- **No key-level retry today**: `pool.next()` is consumed once per request inside each
  provider's `generate()`. A 429 surfaces to the caller; `proxy/router.rs` only retries at
  the deployment level.
- **OpenAPI** is generated from Rust **utoipa** annotations
  ([openapi.rs](../../../litegen-core/src/api/openapi.rs)); SDKs regenerate via
  [`sdks/scripts/regen-all.sh`](../../../sdks/scripts/regen-all.sh) (TS via
  `openapi-typescript`, Python via `openapi-python-client`).
- `rand = "0.8"` is already a dependency of litegen-core.

## Design

### 1. Data model

Add a **plaintext** `routing_config` column to `org_provider_credentials` (Postgres +
SQLite migrations). Stored as JSON text (`TEXT`, not `JSONB`) to avoid the strict sqlx
Postgres decode pitfalls noted in prior work (see `pg-sqlx-strict-decode` memory).

```jsonc
{
  "strategy": "weighted_round_robin",   // | "weighted_random" | "least_busy"
  "failure_mode": "none",                // | "retry_no_cooldown" | "retry_cooldown"
  "cooldown_seconds": 30                  // only consulted when failure_mode = retry_cooldown
}
```

- Column is nullable; `NULL` ⇒ treated as the default config (`weighted_round_robin` /
  `none`). This makes the migration a no-op for existing rows and preserves today's behavior.
- Keys and per-key weights remain in the encrypted `ciphertext` blob, unchanged. Weight is
  intrinsically per-key, so it is edited alongside keys; `routing_config` is the
  provider-wide selection policy.

### 2. Rust types ([litegen-core/src/types/mod.rs](../../../litegen-core/src/types/mod.rs))

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    WeightedRoundRobin,
    WeightedRandom,
    LeastBusy,
}
impl Default for RoutingStrategy { fn default() -> Self { Self::WeightedRoundRobin } }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    None,
    RetryNoCooldown,
    RetryCooldown,
}
impl Default for FailureMode { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RoutingConfig {
    #[serde(default)]
    pub strategy: RoutingStrategy,
    #[serde(default)]
    pub failure_mode: FailureMode,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u32,
}
fn default_cooldown_seconds() -> u32 { 30 }
impl Default for RoutingConfig { /* strategy/failure_mode defaults + 30s */ }
```

`ProviderCredentialInfo` gains `routing_config: RoutingConfig` (always populated; defaults
when the column is NULL) so the dashboard can render current settings on load.

### 3. Selection logic ([providers/mod.rs](../../../litegen-core/src/providers/mod.rs))

The pool is the right home for selection; it already owns the fingerprint-keyed global
registry pattern. Extend the pool to be strategy-aware and add a per-key runtime registry.

- **Global runtime registry**, keyed by `(fingerprint, key_index)`:
  - `cursor: AtomicUsize` (existing, weighted_round_robin only)
  - `in_flight: AtomicUsize` (new, least_busy)
  - `cooldown_until: AtomicU64` epoch-seconds (new, retry_cooldown)
- **Selection** (`fn select(&self, strategy, exclude: &[usize]) -> Option<usize>`):
  - `weighted_round_robin`: existing schedule + cursor, skipping cooled/excluded indices.
  - `weighted_random`: weighted pick via `rand`, over non-cooled/non-excluded keys.
  - `least_busy`: pick the non-cooled/non-excluded key with the minimum `in_flight`
    (ties broken by cursor advance for fairness).
- **In-flight accounting** (least_busy): selection returns an RAII guard
  (`InFlightGuard`) that increments on creation and decrements on `Drop`, so it correctly
  follows the request lifetime regardless of success/error/panic.

If every key is cooled down, selection falls back to ignoring cooldown (better to try a
cooled key than fail outright); this is logged.

### 4. Failure handling / failover

Key-level retry wraps the call that consumes a key. Because `pool.next()` is currently
consumed *inside* each provider's `generate()`, we centralize via a small execution helper
so all providers get failover uniformly without duplicating logic per provider:

```rust
// Pseudocode — exact shape finalized in the implementation plan.
pool.execute(strategy, failure_mode, |key| async move {
    provider.call_with_key(key, &req).await
}).await
```

Behavior:

- `none`: select once, call once, return result (today's behavior).
- `retry_no_cooldown`: on a retryable failure (429 / auth / quota), add the failed index to
  `exclude`, re-`select`, retry. Stop after trying all keys; return the last error.
- `retry_cooldown`: same as above, **and** set `cooldown_until = now + cooldown_seconds`
  for the failed index before re-selecting.

Retryable classification reuses existing provider error categorization (429 / rate-limit /
auth / quota are retryable for failover; deterministic 4xx like bad-request are not). This
is key-level failover and is independent of the deployment-level retry in
`proxy/router.rs`, which is unchanged.

### 5. API

- `CreateProviderCredentialRequest` gains an optional field:
  ```rust
  pub credentials: Option<serde_json::Value>,   // now optional
  pub routing_config: Option<RoutingConfig>,
  ```
  Semantics in `create_org_provider_credential` (still an upsert):
  - `credentials` present ⇒ encrypt + store as today.
  - `credentials` absent ⇒ **keep existing ciphertext/nonce/display_hint** (routing-only
    update). Requires a DB method to update routing_config independently, and the upsert
    must not clobber existing secrets when only routing changes.
  - `routing_config` present ⇒ store it (plaintext). Absent on a first-time create ⇒ store
    default config.
  - Reject the case where the row does not yet exist **and** `credentials` is absent
    (cannot create a credential with no keys).
- `ProviderCredentialInfo` (list response) includes `routing_config`.
- utoipa annotations updated for the new schema components; no new routes required (the
  existing POST handles both key and routing updates).

### 6. Persistence layer

- New trait methods on the DB abstraction:
  - `upsert_org_provider_credential(...)` extended to accept `routing_config` JSON, OR a
    dedicated `update_org_provider_routing_config(org_id, provider, json)` for the
    keys-absent path. (Plan picks the cleaner split.)
  - `list_org_provider_credentials` selects and deserializes `routing_config`.
- Migrations: add `routing_config TEXT` (nullable) to both
  `migrations/postgres/` and `migrations/sqlite/` for `org_provider_credentials`.

### 7. SDKs

- Regenerate via [`sdks/scripts/regen-all.sh`](../../../sdks/scripts/regen-all.sh) after the
  utoipa changes, updating:
  - `sdks/typescript/src/generated/schema.d.ts`
  - `sdks/python` generated client
- New schema surfaces: `RoutingStrategy`, `FailureMode`, `RoutingConfig`,
  `routing_config` on `ProviderCredentialInfo` and `CreateProviderCredentialRequest`.

### 8. Dashboard UI ([ProviderSection.tsx](../../../dashboard/src/components/ProviderSection.tsx))

- Read `routing_config` from the list response; prefill controls on load (no keys needed).
- Add per-provider controls above/near the key rows:
  - **Strategy** dropdown: Weighted round-robin / Weighted random / Least busy.
  - **On failure** dropdown: No retry / Retry next key / Retry next key + cooldown.
  - **Cooldown (s)** number input, shown only when failure_mode = `retry_cooldown`.
- Saving routing config alone POSTs `{ provider, routing_config }` (no `credentials`),
  exercising the routing-only update path. Saving keys continues to send `credentials`
  (and may include routing_config too).

## Testing

- **Unit (selection)**:
  - weighted_round_robin: schedule unchanged vs. current behavior (regression).
  - weighted_random: distribution within tolerance across N draws using a seeded RNG;
    weights respected; cooled/excluded keys never chosen.
  - least_busy: always returns the min-in-flight key; guard increments/decrements correctly
    across success, error, and drop/panic.
- **Unit (failover)**:
  - `none`: one attempt, error propagates.
  - `retry_no_cooldown`: 429 on key A ⇒ key B tried; exhausting all keys returns last error.
  - `retry_cooldown`: failed key is skipped within the window and re-eligible after it;
    all-cooled fallback path covered.
- **API**: routing-only update keeps existing secrets (ciphertext unchanged); first-time
  create without `credentials` is rejected; defaults applied when routing_config omitted.
- **DB**: gated Postgres round-trip test (`LITEGEN_PG_TEST_URL`) for the new column storing
  JSON as TEXT, plus SQLite coverage.
- **SDK**: confirm regenerated types include the new shapes (compile check / type assert).

## Migration & rollout

- Additive, nullable column ⇒ safe forward migration; existing rows behave exactly as today
  (default strategy + `none` failover). No data backfill required.
- No breaking SDK changes: new fields are optional/defaulted.

## Open implementation details (resolved in the plan, not blockers)

- Exact split between extending `upsert_*` vs. a dedicated routing-update DB method.
- Final signature/placement of the `pool.execute(...)` failover helper and how each
  provider adopts it (shared helper vs. lifting key selection into a common call site).
- Error-retryability mapping reused from existing provider error categorization.
