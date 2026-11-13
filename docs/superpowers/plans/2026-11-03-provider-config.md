# Provider Config Everywhere Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every provider config field (`extra_headers`, `model_mapping`, auth) works in every adapter, and the legacy adapter patterns in known-gaps.md are gone.

**Architecture:** Two helpers on the provider config (`mapped_model`, `api_client`) and one header check shared by startup and the admin API; every adapter builds its API client with the operator's headers and applies auth through `auth::apply`; one crate-level conformance test drives every adapter against a mock vendor. The rest are per-adapter fixes, each with its own failing test.

**Tech Stack:** Rust (axum, reqwest, wiremock), the catalog YAML.

**Spec:** `docs/superpowers/specs/2026-11-03-provider-config-design.md`

## Global Constraints

- No commits (the user commits on their own go); work stays uncommitted on `3d-models`.
- No subagents ("just no subagents").
- One cargo at a time, `CARGO_INCREMENTAL=0`; check `pgrep -f 'cargo (build|test|check)'` first.
- `litegen-core/src/mesh/` is not touched.
- Ping peer litegen-20 before an `openapi.rs` change (none planned).
- Never commit `scripts/model-drift/snapshots/`.

## Review Focus

1. A header from `extra_headers` reaching a CDN or object store on a result download (the Hunyuan, Kling, Luma and ByteDance image downloads move off the API client for this).
2. A mapped model id changing the request shape or price instead of only the vendor-facing string (fal endpoint tables, OpenAI gpt-image vs dall-e params, Stability's endpoint per model).
3. An undocumented vendor status failing a job that would have finished: every documented in-progress state must stay in progress.
4. A 429 on a poll (not a submit) turning into a failed job.
5. A `first_frame` and an `init` sent together on the four rows: `max_total: 1` must refuse it.

---

### Task 1: Config helpers and header checks

**Files:** `litegen-core/src/providers/mod.rs`, `litegen-core/src/config/mod.rs`, `litegen-core/src/main.rs`, `litegen-core/src/api/handlers/management/admin.rs`, tests beside each.

**Produces:**
- `ProviderInstanceConfig::mapped_model(&self, model_id: &str) -> Option<&str>`
- `providers::api_client(builder: reqwest::ClientBuilder, extra_headers: &HashMap<String, String>) -> reqwest::Client`
- `providers::check_extra_headers(headers: &HashMap<String, String>) -> Result<(), String>`
- `AppConfig::provider_config_errors(&self) -> Vec<String>`

- [ ] Tests first: `mapped_model_matches_the_catalog_id_then_the_bare_id`; `api_client_sends_extra_headers_and_a_request_header_wins`; `check_extra_headers_refuses_invalid_and_framing_headers` (bad name, CR/LF value, `Host`, `content-length`, `Transfer-Encoding`, `connection`, `proxy-authorization`); `provider_config_errors_name_the_provider_and_the_header`; admin `put_provider_refuses_a_framing_header` (400 `invalid_provider_config`). Run, watch them fail.
- [ ] Implement; `main` refuses to start on `provider_config_errors` (FATAL line per error); the admin PUT checks `settings.extra_headers`. A provider built from config skips an invalid header with a `warn!` (defence in depth).
- [ ] Run: `cargo test -p litegen --lib providers:: config:: && cargo test --test gateway_settings_api` (or the admin test's file). Expected: pass.

### Task 2: Conformance test, then headers, auth and mapping in every adapter

**Files:** `litegen-core/src/providers/config_conformance.rs` (new, `#[cfg(test)]`), every adapter in `providers/{image,video,model3d}/`, `proxy/registry.rs` (factories `pub(crate)`).

**Consumes:** Task 1's helpers.

- [ ] Tests first, in `config_conformance.rs`:
  - `every_adapter_sends_extra_headers_and_auth_to_its_api`: for every provider that serves catalog rows, for each family it serves, build the adapter with `api_base` = one wiremock server (catch-all 503 or a canned accept), `extra_headers: {x-litegen-conformance: yes}`, credentials for its scheme; run `generate` for its first catalog row of that family (required refs supplied in the row's `provider_format`); assert every received request has the header and an auth header for the adapter's `AuthSpec` (`Authorization`, `x-key`, `x-api-key`, `?key=`, SigV4/TC3 `Authorization`).
  - `every_adapter_sends_the_mapped_model_id`: same build with `model_mapping: {<row id>: "litegen-mapped-model"}`; some received request carries `litegen-mapped-model` in its path or body, unless the provider is in `NO_MODEL_ID` (with the reason per entry).
  - `no_adapter_writes_its_own_auth_header`: no adapter source outside `#[cfg(test)]` writes `.header("Authorization"` by hand.
  - `result_downloads_never_carry_extra_headers`: Hunyuan, Kling, Luma and ByteDance image download their result through `fetch_result` (the mock CDN sees no `x-litegen-conformance`).
  Run, watch them fail.
- [ ] Sweep each adapter once: build `client` with `api_client` in `configure`; route the vendor model id through `mapped_model`; replace hand-written auth with an `AuthSpec` + `auth::apply` (openai image/video, stability, replicate image/video, fal image/video/3D `Key`, luma video, runway video); move the four image downloads to `image::fetch_result` with an `asset_client`.
- [ ] Run: `cargo test -p litegen --lib providers::` Expected: pass, including every pre-existing adapter test.

### Task 3: Numeric value sets

**Files:** `api/middleware/validator.rs`, the catalog loader (`capabilities/`), `models/fal.yaml`, `models/hunyuan.yaml`.

- [ ] Tests first: `fps_outside_its_values_is_param_enum_mismatch` (image `steps` and video `fps` too); `a_typed_extra_param_is_checked_against_its_spec`; loader `a_values_entry_outside_min_max_is_refused`; catalog `ltx_2_3_and_hunyuan_i2v_state_their_fps_sets`.
- [ ] Implement one `check_number_values` used by every Int/Float check; `check_extra` checks an allowlisted key that also has an Int/Float spec; the loader check; the two rows' `values`.
- [ ] Run validator, loader and catalog tests. Expected: pass.

### Task 4: `first_frame` on the init rows

**Files:** `models/fal.yaml`, `models/runway.yaml`, `providers/video/fal.rs`, `providers/video/runway.rs`, `tests/refs_api.rs` (or `keyframes_api.rs`).

- [ ] Tests first: each adapter sends a `first_frame` ref as its start image (`promptImage`, `image_url`); `/v1/models/{id}` reports `supports_first_frame: true` for the four rows; a request with both `init` and `first_frame` is a 400.
- [ ] Implement: rows gain `first_frame` (default role) and keep `init`; adapters read `first_frame`, else `init`.
- [ ] Run the adapter tests and the API test file. Expected: pass.

### Task 5: 429s carry Retry-After

**Files:** `providers/video/{luma,google,openai,bedrock}.rs`, `providers/image/{pixellab,bedrock}.rs`.

- [ ] Tests first, one per adapter: a submit answered `429` with `Retry-After: 7` is `ProviderError::RateLimited { retry_after_ms: 7000, .. }`.
- [ ] Implement with `read_vendor_json`.
- [ ] Run the six adapters' tests. Expected: pass.

### Task 6: Undocumented statuses fail; Luma image's poll

**Files:** every polling adapter: video (bytedance, fal, google, hunyuan, kling, leonardo, luma, minimax, openai, pixverse, runway, vidu, bedrock), image (bfl, hunyuan, kling, leonardo, luma, runway, replicate, bytedance if it polls), 3D (fal, meshy, tripo).

- [ ] Tests first: per adapter, a poll answering `{"<status field>": "litegen_unknown_state"}` is `Failed` with an error naming the state (image adapters: the call fails non-retryable, naming it); every documented in-progress state is still in progress (existing tests plus any missing state). Luma image: a 503 page then a completed answer delivers the image; a transport error then completion delivers it.
- [ ] Implement: explicit documented arms (`@see` each) and an `other =>` arm that fails naming the state; Luma image's poll uses `read_poll_json` and asks again after retryable errors.
- [ ] Run `cargo test -p litegen --lib providers::`. Expected: pass.

### Task 7: `extra` never overwrites owned fields

**Files:** `providers/image/hunyuan.rs`, `providers/video/hunyuan.rs`, `providers/image/luma.rs`.

- [ ] Tests first: an `extra` naming an owned field (`Prompt`, `prompt`, `model`…) leaves litegen's value in the body.
- [ ] Implement `OWNED_FIELDS` per adapter and filter the merge.
- [ ] Run the three adapters' tests. Expected: pass.

### Task 8: fal's Platform API stays behind the operator's base

**Files:** `providers/{image,video,model3d}/fal.rs`.

- [ ] Tests first: with `api_base` set to a mock and no `platform_api_base`, `health_check` makes no request outside the mock, is healthy, and says the Platform API isn't checked; with `platform_api_base` set, it calls it (existing tests).
- [ ] Implement in the three adapters (one shared helper in `model3d::fal`).
- [ ] Run the fal tests. Expected: pass.

### Task 9: Docs and the gap list

**Files:** `.claude/skills/provider-standards/references/known-gaps.md`, `.claude/skills/provider-standards/SKILL.md` and `references/auth-and-credentials.md`, `.claude/skills/{image,video,model3d}-provider/…` where they describe these patterns, `docs/` provider config page (or README config section), `litegen.yaml` example if present.

- [ ] Delete the fixed known-gaps entries; document `extra_headers` (where they go, what's refused, unsigned on signing schemes), `model_mapping` (keys, what changes, the no-model-id list), `values` everywhere, `first_frame` on the four rows.
- [ ] Run the doc-adjacent tests (`catalog_conformance`, skills lint if any). Expected: pass.

### Final

- [ ] Full gate: Postgres container, `cargo test --no-fail-fast` with `LITEGEN_PG_TEST_URL`; dashboard vitest/eslint/tsc; SDK suites if `/v1/models` output changed; browser suites if the dashboard changed.
- [ ] Self-review against the spec and the Review Focus; fix Critical/Important with RED→GREEN; ledger every Ruling and deferred minor.
