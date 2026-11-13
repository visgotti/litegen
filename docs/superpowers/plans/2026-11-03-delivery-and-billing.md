# Delivery and Billing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An image job the vendor accepted is always delivered (late, or asynchronously on request), Veo's video is kept, keep-nothing requests a model can't serve are refused before billing, and every varying row prices its variants.

**Architecture:** Resumable image jobs: `VendorJobSink` keeps an optional resume state; each polling image adapter splits its post-poll tail into `finish`, and `poll_job` runs its own poll loop under a one-poll deadline. The handler turns a timed-out (or `Prefer: respond-async`) accepted job into a `processing` image row billed like a video submit; the poller finishes it through the same `build_image_results` and webhook path.

**Tech Stack:** Rust (axum, sqlx, wiremock), the catalog YAML, the TS and Python SDKs.

**Spec:** `docs/superpowers/specs/2026-11-03-delivery-and-billing-design.md`

## Global Constraints

- No commits; no subagents; one cargo at a time with `CARGO_INCREMENTAL=0`; `src/mesh/` untouched; never commit `scripts/model-drift/snapshots/`.
- Ping peer litegen-20 before the `openapi.rs` change (Task 3).
- No new migration (019–022 are this branch's, 018 is the mcp branch's): late-image state lives in `generations.metadata`.

## Review Focus

1. A late image delivered twice (client poll and poller both finishing it): one atomic claim.
2. A late image billed twice, or never refunded when it fails later.
3. The late result differing from what the call would have returned (format, views, access-mode links).
4. A keep-nothing or `preserve` request reaching the async path.
5. A sync caller's 504 now carrying a generation id that never completes (a vendor whose job can't be resumed).

---

### Task 1: Resumable image jobs in the adapters

**Files:** `providers/mod.rs` (VendorJobSink, ImageJobPoll, trait methods), the seven polling image adapters (bfl, kling, hunyuan, leonardo, luma, runway, replicate), `providers/config_conformance.rs`.

**Produces:**
- `VendorJobSink::record_resumable(id, resume: serde_json::Value)`, `VendorJobSink::resume() -> Option<Value>`
- `pub enum ImageJobPoll { Running, Done(GenerationOutput), Failed(String) }`
- `ImageProvider::resumable(&self, model: &ModelSchema) -> bool` (default false)
- `ImageProvider::poll_job(&self, model: &ModelSchema, job: &str, resume: Option<&Value>) -> Result<ImageJobPoll, ProviderError>` (default `Err(InvalidRequest)`)

- [ ] Tests first: conformance `every_resumable_image_job_finishes_through_poll_job` — for each of the seven adapters, a job whose first poll is still running when the deadline passes is then finished by `poll_job` (mock: submit accepted, poll 1 running, poll 2 done with a result PNG served by the mock), returning `Done` with the PNG; a vendor failure is `Failed`; a passing failure is `Err(retryable)`; `resumable()` is true for their rows and false for a synchronous adapter's.
- [ ] Implement: split each adapter's post-poll tail into `finish`; `poll_job` = the adapter's poll loop under `with_deadline(now + 1 ms)` mapped to `ImageJobPoll`; BFL records its `polling_url` as resume state.
- [ ] Run `cargo test -p litegen --lib -- providers::`. Expected: pass.

### Task 2: Late delivery through the poller

**Files:** `proxy/router.rs` (`generate_image*` take an optional wait), `api/handlers/mod.rs` (image handler), `proxy/poller.rs` (image arm), tests in `tests/late_images_api.rs` (new).

- [ ] Tests first (API, mock vendor through the harness): a sync image call that outlasts the vendor timeout answers 504 `vendor_timeout` with `generation_id` and `status: processing`; the poller then completes the row, `GET /v1/generations/{id}` serves the image with the app's access-mode link, the key's webhook fires once, and the charge was taken once (at accept). A vendor failure after the timeout refunds and webhooks. Client polls and the poller don't both deliver (claim).
- [ ] Implement: on `VendorTimeout { vendor_job_id: Some(_) }` from a resumable row with keeping on and no `preserve`, the handler writes the `processing` row (`provider_job_id`, `metadata.image_job {extras, resume, keep}`, `charged_to`), settles the reservation to the price; the poller's image arm calls `poll_job`, builds results with `build_image_results`, writes assets, settles, webhooks; failures refund through `settle_terminal`.
- [ ] Run the new test file and the poller tests. Expected: pass.

### Task 3: `Prefer: respond-async` and `Prefer: wait`

**Files:** `api/handlers/mod.rs`, `api/openapi.rs` (202 answer, `Prefer` header), SDKs (TS and Python: an `ImageGenerationAccepted` type and a `wait_for_generation` helper), `tests/late_images_api.rs`.

- [ ] Ping litegen-20 about the openapi.rs change.
- [ ] Tests first: `respond-async` → 202 with `{id, object: "image.generation", status: "processing", model, created}`, `Location`, `Preference-Applied: respond-async`; `wait=1` on a slow job → 202 after about 1 s; refusals (400 `async_unsupported` for a sync vendor or a `preserve` edit, `async_needs_keeping` for keep-nothing); the poller completes the row.
- [ ] Implement: parse `Prefer` (RFC 7240 tokens, case-insensitive, `wait` capped at the vendor timeout), pass the deadline to the router, answer 202.
- [ ] Regenerate `sdks/openapi.json` and the SDK types (`sdks/scripts/regen-all.sh` or the per-SDK steps used in the storage round), add SDK tests.
- [ ] Run the API tests, the SDK suites and the openapi tests. Expected: pass.

### Task 4: Veo bytes and keep-nothing refusals

**Files:** `providers/video/google.rs`, `providers/mod.rs` (`VideoProvider::hands_out_links`), `providers/video/bedrock.rs`, `api/handlers/mod.rs` (video handler), tests.

- [ ] Tests first: Google video's poll downloads `video.uri` with the key and returns `video_data` (wiremock; size cap; a redirect to a private address refused); a keep-nothing video request for a model whose adapter doesn't hand out links is a 400 `keep_outputs_unsupported` with no vendor call.
- [ ] Implement.
- [ ] Run the tests. Expected: pass.

### Task 5: Variable pricing

**Files:** `models/*.yaml`, `tests/pricing_api.rs` (or `catalog_conformance.rs`).

- [ ] List every row without `variable_pricing` whose `@see` pricing quote (or vendor page) varies by a param; re-read each vendor's pricing page (2026-11-03).
- [ ] Tests first: one per changed row, a non-default variant's quote equals the vendor's price.
- [ ] Add the rules; flat rows stay flat; a price the schema can't express is reported.
- [ ] Run the pricing tests and the catalog conformance. Expected: pass.

### Task 6: Docs and the gap list

- [ ] docs: late and async images (README API section, a `docs/async-images.md` or a section in an existing doc), Veo, keep-nothing refusals; known-gaps.md loses the late-delivery, Veo, keep-nothing and variable-pricing entries; the image-provider skill teaches `poll_job`/`finish`/`resumable`.

### Final

- [ ] Full gate with Postgres; dashboard and SDK suites; self-review against the Review Focus; ledger every Ruling and deferred minor.
