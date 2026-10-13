# Known gaps and legacy patterns

This file lists core behaviors and legacy adapter patterns that a new
integration has to work around or report, as of 2026-10-12. Don't copy them,
and don't paper over them inside a single adapter.

Several sessions are changing this code in parallel. Re-check an entry before
you rely on it. Most entries come with a grep that shows the current state,
because hand-made lists go stale. When you find an entry fixed, delete it from
this file.

## Contents

- [Outputs and storage](#outputs-and-storage)
- [Credentials and polling](#credentials-and-polling)
- [Validation and the catalog](#validation-and-the-catalog)
- [Billing and retries](#billing-and-retries)
- [Legacy adapter patterns not to copy](#legacy-adapter-patterns-not-to-copy)
- [Surfaces that lag the catalog](#surfaces-that-lag-the-catalog)

## Outputs and storage

- **Video results are passed through and never re-hosted.**
  `ProxyRouter::get_video_status` and the poller forward only
  `VideoGenerationPollResult.video_url`; `video_data` (bytes) is dropped.
  - Google Veo's `video.uri` needs the API key to fetch.
  - The Bedrock and OpenAI video adapters returned an `s3://` URI and inline
    bytes respectively. Their catalog rows are retired now.

  *In an adapter:* if the vendor returns bytes, an auth-gated URL or a URL
  that expires quickly, stop and raise it. The fix belongs in shared re-host
  plumbing (Option A in
  `docs/superpowers/2026-09-11-model-drift-decisions.md`), not in one adapter.
- **A completed video with no URL is stored as completed.** The poller writes
  `result_url` exactly as given, and video has no completion guard the way 3D
  has. *In an adapter:* when the vendor reports success but you can't
  extract a URL, return `Ok(Failed)` with an explanation.
- **`url`-form rows may not receive a fetchable URL.** When a caller sends a
  base64 or blob ref:
  - at HEAD, the materializer uploads it to temp storage that is hard-wired to
    `LocalStorage` (`main.rs`), so the vendor gets `local://tmp/refs/<uuid>`;
  - an uncommitted change on 2026-10-12 sends it inline as a
    `data:<mime>;base64,…` URI instead.

  Read `Materializer::convert` to see which version you have. Either way, only
  a caller-supplied public URL is guaranteed to work. *In new rows:* prefer
  `base64`. For a `url` row, check live that the vendor accepts `data:` URIs.
  If it doesn't, declare `form: multipart` and upload the bytes to the
  vendor's file API (Leonardo's `upload_init_image`).
- **The S3 image store picks the file extension from the content type.**
  `S3Store::store` handles png, jpeg and webp, and saves anything else as
  `.png`. An SVG is stored under a `.png` key, although its content type stays
  correct.

## Credentials and polling

- **Video client polls use the platform instance, not the org's BYO
  credential.** `get_video_status` calls `video_provider_for`. For an org with
  only BYO keys, `GET /v1/videos/{id}` fails, while `GET /v1/generations/{id}`
  works because the poller resolves BYO correctly. 3D client polls reuse the
  instance that submitted the job (`Model3dJob.provider`). *In an adapter:*
  `poll_status` must not depend on which instance runs it.
- **A non-retryable poll `Err` reaps the row without a webhook.** The poller's
  `handle_poll_error` calls `reap_generation`, which fails the row but fires no
  webhook. Only a terminal `Ok(…)` takes the webhook path. *In an adapter:*
  return vendor verdicts as `Ok(Failed)`.
- **Config fields only some adapters honor.** No adapter applies
  `ProviderInstanceConfig.extra_headers`. Only a few read `options` (Bedrock
  video, fal 3D) or `model_mapping`. Run
  `grep -rln model_mapping litegen-core/src/providers` to see which.
- **3D `stage_context` is write-once.** Nothing persists a change made
  after submit, so a multi-stage vendor needs core plumbing before its
  adapter can exist. The model3d-provider skill explains what that
  plumbing is.

## Validation and the catalog

- **The validator ignores `capabilities` flags for image and video.** Only
  `GET /v1/models` reads them. A required mode therefore needs a ref role with
  `required: true` plus a refusal in the adapter. For 3D, `validate_model3d`
  refuses a text-only request to a model without `text_to_3d` (uncommitted on
  2026-10-12). Keep the adapter's refusal either way.
- **There are no discrete numeric sets.** You can't say "5 or 10 s".
  Advertise the range, then snap or reject in the adapter, and report it
  under "Can't express".
- **`extra` is unfiltered in lax mode.** `check_extra` returns early when
  `strict` is false, and most adapters merge `extra` *after* writing their
  own fields. In OpenAI's image adapter, `extra.model` overrides the model
  that actually runs. Exceptions: `model3d/fal.rs` skips the keys it maps,
  and `image/ideogram.rs` re-filters against `extra_allowlist`. *In new
  code:* merge `extra` first, or skip the keys you own.
- **Some video rows use `init` for the start frame** (`fal/video`,
  `fal/ltx-2.3`, `runway/gen4-turbo`, `runway/gen4.5`). For them,
  `supports_first_frame` is false. New rows use `first_frame`.

## Billing and retries

- **Async jobs are charged at submit.** Video and 3D bill `base_cost_usd`
  once when submitted, and the quota is settled then. A job that later fails
  is not refunded.
- **`n` is uncapped, and most image adapters forward it.** The vendor
  generates and bills N images; litegen returns and bills one. To see the
  current offenders, run
  `grep -rn "base\.n" litegen-core/src/providers/image/`. On 2026-10-12 that
  showed bedrock, fal, google, ideogram, kling, leonardo, minimax, openai,
  recraft and stability (`samples`). *In new code:* don't forward it.
- **A timeout re-submits a billed job.** On direct dispatch the router wraps
  each `generate()` in a 120 s timeout and retries up to twice.
  `ProviderError::Timeout` and `RateLimited` are retryable by type.
  - Image adapters that poll internally run for 180–240 s: bfl, replicate,
    kling, leonardo, hunyuan, luma and runway.
  - Most of them return `Timeout` when they run out of attempts, and mark
    transport errors during polling as retryable.

  All of these paths re-submit a job the vendor already accepted. *In new
  code:* give `generate()` one wall-clock deadline well under 120 s, and
  make errors after acceptance non-retryable `RequestFailed`.
- **429 handling is inconsistent.** Most adapters set
  `retryable: status >= 500`, so a 429 on submit isn't retried. fal 3D and
  MiniMax video do treat 429 as retryable, and `RateLimited` is never
  constructed. *In new code:* treat a 429 on submit as retryable.
- **Only OpenAI image's `estimate_cost` scales by size and quality,** so its
  `/v1/images/cost` quote differs from what the router bills.
- **Flat billing under-charges expensive variants.** Per-megapixel,
  per-second and per-resolution prices are all billed at the default
  request's price. Pricing evidence makes this visible; fixing it is a
  product decision.

## Legacy adapter patterns not to copy

- **Hand-written auth headers**, instead of `AuthSpec` + `auth::apply`:
  openai (image and video), stability, replicate (image and video), fal
  (image, video and 3D), luma video, runway video and google image.
- **Hosts hard-coded outside `api_base()`.** Examples:
  - the poll fallbacks and health check in `video/fal.rs`;
  - the health checks in `image/openai.rs`, `image/google.rs` and
    `image/fal.rs`;
  - OpenAI image's `api_base`, which is a full generations URL.

  Find more by grepping for quoted `https://` strings outside the
  `api_base()` default.
- **`_ =>` arms that substitute a default model:**
  - leonardo: MOTION2, or a default image model;
  - runway: `gen4_image` and `gen4_turbo`;
  - luma: `ray-2` and `photon-1`;
  - fal: `flux/dev` and LTX;
  - google image: gemini-3.1-flash-image;
  - replicate video: AnimateDiff.
- **Credentials sent to a vendor-chosen URL.** `image/bfl.rs` sends `x-key` to
  whatever `polling_url` comes back. `model3d/fal.rs` shows the fix:
  `trusted_queue_url`, plus SSRF-checked downloads.
- **Unknown vendor states mapped to `Processing` or `Pending`.** Most video
  adapters do this, including `video/fal.rs`, and such a job polls until the
  reaper's 2 h maximum age.

## Surfaces that lag the catalog

- `apps/landing/src/components/infra-flow/capabilities.generated.ts` and
  `FALLBACK_CAPABILITIES` say fal has `model3d: false`. Regenerating the
  first needs a running gateway (`sync-capabilities.mjs`). `site.ts`
  `PROVIDERS` has no 3D flag. `derive-capabilities.test.mjs` hard-codes 18
  providers and requires an image or video output for each.
- The README's provider sections and intro predate most vendors and 3D, so
  check them before quoting them.
- `apps/price-api` seeds 7 of the 18 providers.
- `cargo clippy --all-targets -- -D warnings` isn't clean on this branch,
  because of pre-existing warnings in code that provider work didn't write.
- `models/bedrock.yaml` was deleted on 2026-10-12, after Nova's end of life.
  Bedrock is still registered for image and video, and still has a dashboard
  credential form, but it serves no models.
