# Known gaps round 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Six known gaps closed. Videos with nothing to keep fail at once. Reaps send webhooks. Client video polls use the job's own instance. Image adapters poll as long as litegen waits. 429s are retried in Google, Luma and OpenAI image. SVG and GIF keep their formats.

**Architecture:**
- `rehost_video` returns `RehostError { Permanent, Transient }`.
- `reap_generation` takes a `Notify` (webhook client, mode, presenting) and dispatches.
- The video job map stores `Arc<dyn VideoProvider>`.
- `ImageExtras.deadline: Option<Instant>` is set by the router.
- One `image_format(content_type)`.

**Tech Stack:** Rust (axum, tokio, wiremock).

**Spec:** `docs/superpowers/specs/2026-11-02-known-gaps-round-1-design.md`

## Global Constraints

- **Permanent re-host failures:** no data and no URL, a non-public URL, a download 4xx, too large. Everything else is transient.
- **One webhook per terminal transition,** from the observer whose guarded write wins.
- **Adapters without a deadline keep today's budget.**
- **Shared checkout:**
  - no commits;
  - `CARGO_INCREMENTAL=0`, one cargo at a time;
  - `src/mesh/` untouched;
  - no subagents.

## Review Focus

1. **A transient copy failure** (a 5xx, or litegen's store down) must not fail the video: it keeps retrying. Tested in Task 1.
2. **A reap racing a client poll's terminal write** must send exactly one webhook. Tested in Task 2 (the loser of the guarded write sends nothing).
3. **A video submitted through a route** keeps the deployment instance that took it. Tested in Task 3.
4. **A deadline already past when `generate()` starts** must fail at once, not after one full poll interval. Tested in Task 4.

---

### Task 1: A video with nothing to keep fails at once

**Files:** `src/proxy/router.rs` (`rehost_video`, `RehostError`, the client poll path), `src/proxy/poller.rs` (the video branch), `src/providers/video/mock.rs` with `models/mock.yaml` (`mock/empty-video`: finishes with neither bytes nor URL).

**Tests:**
- unit: `rehost_failures_are_permanent_or_transient` (wiremock 404, 503, too large; no data and no URL; a non-public URL);
- API: `a_video_with_nothing_to_keep_fails_at_the_first_poll`. Client poll → `failed`, an error naming the cause, cost refunded to 0, `GET /v1/generations/{id}` failed;
- poller: `the_poller_fails_a_video_with_nothing_to_keep_at_once` (`poll_once` on a row of `mock/empty-video`).

### Task 2: Every reap sends its webhook

**Files:** `src/proxy/poller.rs`: `reap_generation(db, gen_id, reason, notify: &Notify)`; every call site passes the tick's notify; `Notify { wh_client, mode, presenting: Option<Presenting> }`.

**Tests** (poller unit tests with wiremock webhook receivers):
- `a_non_retryable_poll_error_reaps_with_a_webhook`;
- `a_row_past_max_age_is_reaped_with_a_webhook`;
- `a_reap_that_loses_the_race_sends_nothing`.

### Task 3: Client video polls use the job's instance

**Files:** `src/proxy/router.rs`: `VideoJob { handle, usage, provider: Arc<dyn VideoProvider> }` replaces `TrackedJob<VideoGenerationHandle>`. Direct and routed submits store the instance; `get_video_status` uses it.

**Test** (router tests): `a_video_is_polled_with_the_instance_that_took_it`. The platform mock fails every poll; a BYO instance (app credentials) answers; the client poll succeeds through the BYO instance. Same for a routed submit.

### Task 4: Image adapters poll as long as litegen waits

**Files:**
- `src/providers/mod.rs`: `ImageExtras.deadline`, plus a helper `PollBudget::new(deadline, default) -> PollBudget` with `next(&mut self) -> bool` (sleeps the interval, false when out of time; it never sleeps past the deadline).
- `src/proxy/router.rs`: set `extras.deadline` per attempt.
- The seven adapters (bfl, replicate, kling, hunyuan, leonardo, luma, runway).

**Tests:**
- `the_poll_budget_ends_at_the_deadline_or_the_default` (unit);
- bfl and replicate wiremock: `an_endless_pending_ends_at_litegens_deadline` (deadline 3 s → error within 3–5 s).

### Task 5: 429s retried in Google, Luma and OpenAI image

**Files:** `src/providers/image/{google,luma,openai}.rs`.

**Tests:** per adapter, `a_429_is_retryable` (wiremock 429 + `Retry-After`).

### Task 6: SVG and GIF keep their format, plus docs

**Files:**
- `src/proxy/storage.rs`: `pub fn image_format(content_type) -> &'static str`; `S3Store::store` and `keep_image_result` use it.
- `known-gaps.md`: closed entries removed, round-2 items kept.

**Tests:**
- `image_formats_follow_the_content_type` (unit);
- `an_svg_result_is_kept_as_svg` (API, if a mock can answer SVG; else a unit test on `keep_image_result`'s key and format).

**Final:** self-review, the full gate (Postgres), the dashboard suites, the ledger. No commit.
