# Multi-Tenant Phase 2 — BYO Provider Credential Threading

> Executes the Phase-2 boundary of `docs/superpowers/specs/2026-06-03-multi-tenant-hosted-platform-design.md`. Phase 1 (orgs/apps/keys/isolation/encrypted cred *storage*) is on `main`. This phase makes a stored per-app provider credential actually *used* for the upstream call.

**Goal:** At generation time, resolve the calling app's stored encrypted provider credential, decrypt it, and use it for the upstream provider call instead of the global env credential. Fall back to the global credential when the app has none; return `400 provider_not_configured` when neither exists.

**Architecture (chosen — lowest blast radius, no per-provider edits):** Providers read their credential from a `ProviderInstanceConfig` baked in at boot via `configure()`. So we resolve the per-app `ProviderCredentials` in the handler (DB lookup + AES-GCM decrypt), pass it down to the router, and the **registry builds a per-request provider instance `configure()`d with the app's credential** (reusing the existing construction match, refactored into a factory). The cached global instance is used only when no app credential is present. This works for every auth scheme (bearer/SigV4/TC3/Kling) because it goes through the same `configure()` path providers already use — no provider impl changes.

## Files

- `litegen-core/src/proxy/registry.rs` — extract the per-name construction match into `build_image_provider(name, &ProviderInstanceConfig) -> Option<Box<dyn ImageProvider>>` + `build_video_provider(...)`; store each registered provider's `ProviderInstanceConfig` (a `RwLock<HashMap<String, ProviderInstanceConfig>>`) so per-request overrides keep the global non-credential fields (api_base, model_mapping, extra_headers); add `image_provider_for_request(name, override: Option<ProviderCredentials>)` + `video_provider_for_request(...)` that return the cached Arc when `override` is None, else build a fresh `Arc<dyn …>` from `base_config.with_credentials(override)`.
- `litegen-core/src/providers/mod.rs` — small helper on `ProviderInstanceConfig`: `fn with_credentials(&self, creds: ProviderCredentials) -> Self` (clone, replace `.credentials` + `.api_key` from `creds.api_key`). `ProviderCredentials::from_json(&serde_json::Value) -> ProviderCredentials` (map api_key/key_id/key_secret/region/extra).
- `litegen-core/src/proxy/router.rs` — `generate_image`/`generate_video`/`estimate_*` gain `app_creds: Option<ProviderCredentials>`; use `*_for_request(name, app_creds)`. Latency/circuit-breaker/cache stay keyed by provider name (unchanged).
- `litegen-core/src/api/handlers/mod.rs` — in `generate_image`/`generate_video` + cost handlers: resolve `app_creds` = if `ctx.app_id` + `state.secrets_key` and `db.get_provider_credential(app_id, schema.provider)` is `Some((ct,nonce))` → `decrypt` → `serde_json::from_slice` → `ProviderCredentials::from_json`. If the resolved provider has neither an app credential NOR a registered global instance → `400 provider_not_configured`. Pass `app_creds` into the router call.
- `litegen-core/src/proxy/poller.rs` + `main.rs` — video status polling runs in the background without request context: give `spawn_poller` the `secrets_key`; in `poll_once`, for each active generation resolve the app credential from `gen.app_id` + `gen.provider` (DB + decrypt) and call `registry.video_provider_for_request(gen.provider, app_creds).poll_status(handle)`.

## Decisions

- **Fallback order:** per-app credential → global env credential → `400 provider_not_configured`. (Single-tenant/mock unaffected: mock needs no creds; single-tenant uses the global env credential as today.)
- **Per-request instance cost:** building a provider per request when an app credential is present is acceptable for Phase 2 (provider structs are cheap; they hold a `reqwest::Client`). A shared-client optimization is a follow-up; note it, don't block.
- **Stored cred shape:** Phase-1's create endpoint stores `serde_json::to_vec(credentials)` where `credentials` is the request JSON (e.g. `{"api_key":"sk-…"}`, or `{"key_id":…,"key_secret":…,"region":…}` for signing schemes). `from_json` maps those fields.

## Tasks (TDD; keep `cargo test` green; commit per task)

1. **Registry factory + per-request provider** — refactor construction into `build_image_provider`/`build_video_provider`; store configs; add `*_for_request`; `ProviderInstanceConfig::with_credentials`. Unit test: `image_provider_for_request("openai", Some(creds{api_key:"K"})).api_key() == "K"`, and `None` returns the global/cached instance.
2. **Credential JSON parse** — `ProviderCredentials::from_json`. Unit test round-trips `{"api_key":"sk-x"}` and `{"key_id":"a","key_secret":"b","region":"us"}`.
3. **Router threading** — add `app_creds` param; route to `*_for_request`. Update all call sites (handlers + tests) to pass `None` by default. Keep green.
4. **Handler resolution + 400** — resolve app creds in generate/cost handlers; `400 provider_not_configured` when neither app nor global. Integration test (`multitenant_api.rs`): a hosted app with NO cred for `openai` calling `POST /v1/images/generations {model:"openai/…"}` → `400 provider_not_configured`; with the mock model still → 200 (mock needs no cred).
5. **Poller per-app polling** — thread `secrets_key` into `spawn_poller`; resolve per-gen app cred in `poll_once`. Keep the poller's existing behavior when `app_id`/cred absent (global instance).
6. **Verify** — `cargo test` (lib + `multitenant_api`), `cargo clippy`, README note that BYO keys are now used at generation time. Optionally a `wiremock`-backed test asserting the per-app key reaches the upstream `Authorization` header.

## Out of scope (later phases)
Phase 3 (Redis/object-storage scaling), Phase 4 (DO infra). Shared per-request HTTP client reuse (perf). UI for testing a stored credential.
