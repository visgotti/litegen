---
name: video-provider
description: Implement or update video generation for a litegen provider. Use it whenever the task touches the video family, such as adding a video vendor, adding video support to a vendor litegen already has for images or 3D, adding, updating, re-pricing or retiring a video model in models/*.yaml, acting on a /model-drift decision for a video model, or fixing an adapter in litegen-core/src/providers/video/ (submit, polling, durations, keyframes, status mapping). Use it even when the request only names a vendor or model ("add Veo 4", "Kling 3 has new durations", "support Hailuo 3"). It covers the VideoProvider adapter, stateless polling, the catalog row, auth and dashboard credentials, registry wiring, tests, live verification and drift hooks, and hands off to image-provider or model3d-provider when the vendor does more than video.
---

# Video providers

**Read `.claude/skills/provider-standards/SKILL.md` first.** It holds the rules
every provider shares: auth and credentials, `api_base`, vendor-doc
citations, pricing, retries, tests and drift hooks. This skill adds what is
specific to video. If the same vendor does, or will do, images or 3D, also
read [Vendors with more than one family](#vendors-with-more-than-one-family).

## Pick your path

| You're asked to | Path |
|---|---|
| Add a video model for a vendor litegen already serves video for | [A](#a-model-on-an-existing-video-adapter) |
| Update a carried video model (durations, resolutions, price…) | `provider-standards/references/touchpoints.md` section B, plus [Writing the adapter](#writing-the-adapter) if the request shape changes |
| Retire or rename a video model | `provider-standards/references/touchpoints.md` section C |
| Add video generation for a vendor litegen has for images or 3D | [B](#b-video-for-a-vendor-litegen-already-has) |
| Add a vendor litegen doesn't have yet | [C](#c-a-new-video-vendor) |
| Fix a video adapter | Write the failing wiremock or conformance test first, then fix the adapter, then work through [Done](#done) |

## How video generation works here

- **The contract is asynchronous** (`litegen-core/src/providers/mod.rs`):
  - `VideoProvider::generate(model, base, extras, materialized)` submits the
    job and returns a `VideoGenerationHandle { provider_job_id, provider,
    model }`.
  - `poll_status(handle)` returns `VideoGenerationPollResult { status,
    progress, video_url, video_data, content_type, error, metadata }`.
- **`generate()` only submits.** It runs under the router's 120 s attempt
  timeout and up to two retries. A retry is a new, billed vendor job, so
  return as soon as the vendor accepts the job. A vendor whose only API
  blocks until the video is done doesn't fit this contract (the Gemini Omni
  deferral explains why). Raise it with the user instead of working around
  it.
- **Two observers poll the same job.**
  - Client polls go through `ProxyRouter::get_video_status`, using the
    *platform* instance (see known-gaps.md).
  - The background poller (`proxy/poller.rs`) uses a fresh instance built
    with the org's BYO credential. It rebuilds the handle from the DB row,
    which keeps **only `provider_job_id`, `provider` and `model`**, and it
    may run after a restart.
- **The output is a URL.** The poller stores `video_url` as the row's
  `result_url` and fires the webhook. Nothing re-hosts video, and
  `video_data` is dropped today. The URL you return is the one the customer
  gets.
- **Inputs.**
  - `base`: prompt, `negative_prompt`, `seed`, `extra`.
  - `extras: VideoExtras`: `duration_seconds` (always set: the caller's
    value, or the row's `default`, or 5.0), `aspect_ratio`, `resolution`,
    `fps`, `extra`.
  - `materialized.refs`: `(role, form)` pairs.
- **Vendor flows you'll meet:**
  - REST task APIs: submit, then `GET /tasks/{id}` (Vidu, Runway, Kling,
    PixVerse, MiniMax, Luma, ByteDance).
  - Long-running operations: submit, then `GET /{operation name}` (Google
    Veo `:predictLongRunning`).
  - Queues: submit, then status, then result (fal).
  - Async invoke with output to the customer's S3 (Bedrock Nova Reel).
  - RPC actions with TC3 signing (Tencent Hunyuan `Submit…Job` /
    `Describe…Job`).

## A. Model on an existing video adapter

1. Read the vendor's API page, request schema and pricing for this model,
   and note the dates. List every endpoint the model can be routed to:
   text-to-video, image-to-video, start-end frames and reference-to-video.
2. Write the conformance facts first (`catalog_conformance.rs`): the
   duration bounds, resolution and ratio enums *valid for every advertised
   mode*, the prompt limit, the required roles, and the capabilities the
   model must not claim. Watch them fail.
3. Add the row to `models/<p>.yaml`, starting from the template in
   `references/adapter-template.md`. Declare a `duration_seconds` `default`,
   which is mandatory when the minimum is above 5 s.
4. In `providers/video/<p>.rs`, add the resolution arm with its `@see`
   triplet, and gate every field that differs per model or endpoint. If the
   vendor accepts only discrete durations, add or extend the snapping or
   rejection and test it.
5. Map the id in `scripts/model-drift/providers/<p>.mjs` to **every** vendor
   id the adapter sends for it, as an array when there is more than one
   endpoint, and make sure each endpoint's schema is snapshotted.
6. Run the checks in [Done](#done).

## B. Video for a vendor litegen already has

Here the vendor is already integrated for images or 3D, and you're adding
the video family.

1. Read the vendor's existing adapter(s) and YAML. Reuse its provider name,
   `AuthSpec`, credential handling, `api_base` default (when one host serves
   both families) and env vars. Add no new provider name or env var.
2. Create `providers/video/<p>.rs` and add `pub mod <p>;` to
   `providers/video/mod.rs`. Import the vendor's shared `pub(crate)`
   helpers, as `video/bytedance.rs` imports `image::bytedance::ark_model_id`
   and `video/leonardo.rs` imports `image::leonardo::upload_init_image`. If a
   helper you need is private to the image adapter, make it `pub(crate)`
   there; don't copy it.
3. In `proxy/registry.rs`, add the import, an arm in `build_video_provider`,
   and the name in `VIDEO_PROVIDERS`. The dashboard's credential form gains
   "video" with no UI change.
4. Add the video rows to the vendor's `models/<p>.yaml` in a video section.
   Keep ids distinct from the image rows; prefix them (`<p>/video-…`) if the
   vendor reuses model names, as Kling does.
5. Test that the vendor's stored dashboard credential configures the new
   adapter:
   `registry.video_provider_for_request("<p>", Some(ProviderCredentials::from_json(&json!({"api_keys":[{"key":"k"}]})))).await`
   must return an instance that is configured.
6. Do everything in path A for each model, then wire the rest from
   `provider-standards/references/touchpoints.md` section D: the drift
   sources for the video endpoints, the landing flags and syncs, the README
   and a live test.

## C. A new video vendor

1. Confirm the vendor has a self-serve, first-party API, how its keys are
   issued, and its auth scheme. If the scheme isn't covered in
   `provider-standards/references/auth-and-credentials.md`, it needs a new
   `AuthSpec` variant with its own tests.
2. Check how the result is delivered **before** you write anything. It might
   be a public CDN URL, a URL that expires, a URL that needs auth, bytes
   inline, or a write to the customer's bucket. Anything other than a
   long-lived public URL hits the re-hosting gap (known-gaps.md), so tell
   the user before you build.
3. Build the adapter from `references/adapter-template.md`, with its tests
   written first.
4. Wire everything in `provider-standards/references/touchpoints.md`
   sections D and E.
5. If the vendor also does images or 3D, agree the scope with the user, and
   build the video half so the other halves can reuse it.

## Writing the adapter

The full skeleton is in `references/adapter-template.md`. What follows is
specific to video.

**Submit**

- **Choose the endpoint from the refs, explicitly.** Vidu's ladder is a good
  model:
  - a `reference` role → reference-to-video;
  - a `last_frame` plus a start image → start-end;
  - any image → image-to-video;
  - otherwise text-to-video, but only if the row advertises `text_to_video`.
    If it doesn't, return `InvalidRequest` naming the role to attach.
- **Order positional images yourself.** If the vendor reads an array by
  position (start, then end), sort by role rather than trusting the order
  the caller sent.
- **Duration.**
  - Send the vendor's type and spelling: Luma `"5s"`, Sora a numeric string,
    Runway an integer.
  - If the vendor takes discrete values ("5 or 10", "4, 6 or 8"), the YAML
    can only state a range. Snap deterministically to an allowed value
    (vidu's `resolve_duration`), or return `InvalidRequest` before
    submitting. Test the snapping, and report it under "Can't express".
  - Don't send `duration` to an endpoint whose schema has no such field (fal
    `EndpointSpec`).
- **Resolution, ratio and fps.**
  - Map each value to the vendor's preset names (Leonardo `RESOLUTION_720`).
  - If no value was requested, choose a default that is valid for the
    specific model (Leonardo's Kling models take only `RESOLUTION_1080`).
  - Never derive a value outside the vendor's enum.
- **Audio, camera and loop switches** are `extra` passthroughs. Allowlist
  them only on rows whose endpoint schema has them.
- **Vendor callbacks and webhooks.** litegen fires its own webhooks from the
  poller. Don't configure a vendor callback URL as a substitute.
- **Return as soon as the vendor accepts.** Check that the response carries
  the job id before you return a handle. A missing id is a non-retryable
  error with the body attached.

**The handle: make polling stateless**

- Put into `provider_job_id` everything `poll_status` needs, because the
  poller has nothing else:
  - the plain task id, when the poll URL depends only on it (`vidu`,
    `pixverse`);
  - `{kind}/{task_id}`, when the poll path differs by endpoint (`kling`);
  - a JSON object, when you need the endpoint and the vendor's status or
    result URLs (`fal`: `{"request_id", "endpoint", "status_url",
    "response_url"}`).
- If the poll URL comes from the vendor, store it in the handle and use it
  as given. Send your key to it only if its host is the vendor's API host or
  the `api_base` host; otherwise fall back to the documented path built from
  `api_base()`. Build every fallback from `api_base()`: `video/fal.rs`
  hard-codes `queue.fal.run` there, which is a known gap.
- Never use `self` state or caches to poll. The instance that polls may not
  be the one that submitted.

**Poll**

- Read the body through `super::read_poll_json(resp, "<p>")`. It turns an
  HTTP 4xx into a terminal error, a 5xx, 408 or 429 into a retryable one, and
  a non-JSON body into an error. Without it, a revoked key or a purged job
  reads as "still processing" forever.
- **Map every documented state.**
  - Success maps to `Completed`, *only when you have a URL*. If the vendor
    reports success but you can't find one, return `Failed` with the body,
    because the poller would otherwise store a completed row with no
    result.
  - Failed, cancelled, expired and moderated map to `Failed`, with the
    vendor's reason in `error`. Runway's `CANCELLED` once polled until the
    timeout.
  - Queued and running map to `Pending` and `Processing`.
  - For an undocumented state, return a *retryable* error that names the
    state, not `Processing`. The poller asks again every 5 s, logs a warning
    each time, and reaps the row after 2 h. That makes the problem loud
    without failing a job that may still finish. If the vendor documents a
    closed enum, a non-retryable error is justified, as `model3d/fal.rs`
    does. `video/fal.rs` still maps unknown states to `Pending`; don't copy
    that.
- **Report progress honestly.** Use the vendor's percentage when it gives
  one, a fixed mid value otherwise, and 100 on completion.
- **The output URL** has to be fetchable by the customer without vendor
  credentials, and should stay alive long enough to download. If it isn't,
  don't ship silently. Veo's auth-gated `video.uri` is the current case;
  Bedrock's `s3://` output and Sora's bytes were earlier ones. Set
  `content_type` (usually `video/mp4`).
- **Return vendor verdicts as `Ok(Failed)`, never as `Err`.** Only a
  terminal `Ok` result goes through the poller's normal tail, which fires
  the customer's webhook. A retryable `Err` leaves the row in flight for the
  next tick. A non-retryable `Err` makes the poller *reap* the row as failed
  without any webhook. Use it only when the job's state can never be
  learned: a 401 or 404 on the status call, or a malformed handle.

## The catalog row

A full annotated template is in `references/adapter-template.md`. The parts
specific to video:

- **`capabilities`**: `text_to_video` and `image_to_video`, exactly the modes
  the adapter routes. A model that only does image-to-video declares just
  `image_to_video` and makes `first_frame` `required: true, min_count: 1`.
- **Roles**: `first_frame` and `last_frame`. Use these exact names, because
  they set `supports_first_frame` and `supports_last_frame` in
  `GET /v1/models`; don't use `init`. Add `reference` for
  reference-to-video and `subject` for a subject reference.
- **One list serves every mode.** The `aspect_ratio`, `resolution` and
  duration range must be valid on every endpoint the row can reach.
  MiniMax and Runway have conformance tests for exactly this. If the modes
  differ too much, split the model into two rows.
- **`duration_seconds`** is `float` (most vendors) or `int` (Runway). Declare
  `min`, `max` and a `default`.
- **Price** is per second × the default duration at the default
  resolution. Show the arithmetic, and list the expensive variants (1080p,
  10 s, audio).

## Tests

Write these in the adapter's `#[cfg(test)] mod tests`, using the idioms in
`provider-standards/references/testing.md`:

- [ ] **Submit, for each endpoint the row can reach** (text-to-video,
      image-to-video, start-end, reference): the path, the auth header
      (from a pooled key in one test), every body field name, type and value
      (duration's type included), and the handle's `provider_job_id`.
- [ ] **Endpoint choice from roles**, including a refusal: a text-only
      request to an image-to-video-only row returns `InvalidRequest` with no
      HTTP call. Mount nothing and assert zero received requests.
- [ ] **Positional ordering** of start and end frames when the vendor reads
      by position.
- [ ] **Duration snapping or rejection** across in-range values that aren't
      allowed.
- [ ] **Fields an endpoint lacks** are absent, with the input set.
- [ ] **Every documented status** maps correctly: success with a URL,
      success without a URL (→ `Failed`), failed with its reason, cancelled
      or expired (→ `Failed`), queued and running.
- [ ] **A poll HTTP 401 or 404** is non-retryable, a 5xx is retryable, and an
      HTML body is an error.
- [ ] **A handle round trip**: a handle rebuilt from `provider_job_id` alone
      polls correctly. Construct `VideoGenerationHandle` by hand, the way
      the poller does.
- [ ] **Resolution ↔ catalog**: every `media_type: video` row for the
      provider resolves (iterate the catalog).
- [ ] **Conformance facts** for each enum, bound and required role.
- [ ] **A live test** `live_<p>_video` using `drive_video`. Then fetch the
      returned URL *without credentials* and check it serves `video/*`.

## Vendors with more than one family

Most vendors serve several families, and the customer's one dashboard key
has to work for all of them.

- **The vendor already has images or 3D in litegen.** Follow path B. If
  you're also changing the image half, do that half with the
  `image-provider` skill, and the 3D half with `model3d-provider`. Don't
  improvise a family's adapter from this skill.
- **The vendor will get images or 3D later.** Build for it now:
  - put vendor-shared logic (id translation, upload helpers, error-envelope
    parsing, host or region selection) in `pub(crate)` functions;
  - pick video ids that won't collide with future image ids;
  - keep one drift definition per vendor;
  - name the planned family in the YAML header comment and in your report,
    so whoever builds it starts from `image-provider` path B.
- **Keep the halves consistent.** Use the same `AuthSpec`, the same
  `api_base` default and meaning, the same credential, env var and error
  parsing. Several legacy video adapters hand-write their auth (luma,
  runway, fal, openai, replicate) even where the image sibling uses
  `AuthSpec`. Match the standard, not the sibling, and report any mismatch
  you leave in place.

## Done

- [ ] Everything under "Before you report" in
      `provider-standards/references/testing.md`.
- [ ] The touch points for your change, checked against
      `provider-standards/references/touchpoints.md` (whichever of sections
      A–E applies).
- [ ] Any new video param × provider cells added to
      `docs/superpowers/param-mapping-matrix.md`.
- [ ] A report in the shape given under Finish in provider-standards. Say
      whether the background poller (not just client polls) drove a job to
      completion, and whether the URL was fetched without credentials.
