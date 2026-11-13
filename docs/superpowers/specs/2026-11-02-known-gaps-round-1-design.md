# Known gaps, round 1 — design

Status: the third part of the operator's 2026-11-02 request ("look into these
known-gaps and resolve them intuitively"), started on their "continue". The
design calls are mine. This round takes the gaps in
`.claude/skills/provider-standards/references/known-gaps.md` that cost a
caller money, time or a missing notification and that core can fix without
vendor research. Pricing rules, legacy adapter refactors, landing-site data
and the remaining storage features (passthrough, IAM roles, retention,
revocation) are later rounds.

## The gaps and the fixes

### 1. A finished video with nothing to keep stalls for 2 h

**Today:** when a vendor reports a video `Completed` with neither bytes nor a
URL litegen can fetch, `rehost_video` fails. Every observer treats that as
"try again". The poller retries each tick and reaps the row after 2 h, with
no webhook.

**Fix:** `rehost_video` says which kind of failure it hit.

- **Permanent:**
  - the vendor returned neither the video nor its URL;
  - the URL isn't a public http(s) URL;
  - the download answered a 4xx;
  - the video is over the size litegen keeps.
- **Transient:** a connection failure, a timeout, a 5xx, or litegen's own
  store failing.

A permanent failure ends the generation **at once**, in whichever observer
sees it (the poller or a client's `GET /v1/videos/{id}`). It goes `failed`
with the reason, is refunded, and the webhook fires. A transient one is
retried as today, then reaped at the maximum age, with its webhook (see 2).

### 2. A reaped generation sends no webhook

**Today:** `reap_generation` fails the row and refunds it, but fires no
webhook. This covers a non-retryable poll error, a provider no longer
configured, no vendor job id, past the maximum age, and a video that could
not be kept. A caller relying on webhooks waits forever.

**Fix:** every reap that wins its guarded write dispatches the terminal
webhook, with the row as settled. This is the same delivery, signature and
URL presenting as any other terminal transition.

### 3. Client video polls use the platform credential

**Today:** `get_video_status` builds the provider from the platform config
(`video_provider_for`). An org on its own key (BYO) is polled with the
platform's, which fails, and it then reads the row the poller advances, up to
a tick late.

**Fix:** the video job map keeps the instance that took the job, as the 3D
map does (`Model3dJob.provider`), whether the job went direct or through a
route. A client poll uses it. A poll for a job no longer in the map (after a
restart) answers from the row, as today.

### 4. An image job can outlast litegen's wait, or litegen can outlast the adapter's

**Today:** seven image adapters poll a fixed 90–120 times every 2 s: bfl and
replicate for 4 min, kling, hunyuan, leonardo, luma and runway for 3. The
router waits `vendor_timeout` (120 s by default, now a live setting up to an
hour). Raising the timeout for a slow model doesn't help, because the adapter
gives up at its own 3–4 min.

**Fix:** the router hands each call its deadline (`ImageExtras.deadline`, the
start of the attempt plus the vendor timeout). The adapters poll until it.
Without one (an adapter called directly, in tests), they keep today's budget.

Still open: a job that outlasts the vendor timeout is billed and not
delivered. Delivering it late needs resumable image polling, which is a
later round. The guide says to raise `vendor_timeout_seconds` for slow
models.

### 5. A 429 is never retried in Google, Luma and OpenAI image

**Today:** they mark only `status >= 500` retryable.

**Fix:** they read answers with `read_vendor_json`, where the shape allows.
A 429 then becomes `RateLimited` (with `Retry-After`) or a retryable
refusal, so retries back off and honour it (`docs/gateway-settings.md`).

### 6. An SVG or a GIF is kept as `.png`

**Today:** `S3Store::store` maps only png, jpeg and webp, and saves anything
else under `.png`. The kept image-result copy names an SVG `image_{i}.png`
with `format: "png"`.

**Fix:** one function maps an image content type to its extension and
format: png, jpeg→jpg, webp, gif, svg+xml→svg. Both use it.

## Non-goals (later rounds)

- Delivering an image job after the vendor timeout.
- `extra_headers` honoured by every adapter (each builds its own client: a
  sweep of about 30 adapters).
- Unknown vendor states mapped to `Processing`.
- Variable pricing rules for the remaining rows.
- Hand-written auth headers.
- fal's hosts outside `api_base()`.
- `extra` merges that don't skip owned fields.
- The landing site's provider data.
- The storage leftovers.

## Anti-goals

- **Failing a video whose copy failed for a passing reason** (a network
  blip, our own store down): those keep retrying.
- **Two webhooks for one terminal transition.** Only the observer whose
  guarded write wins dispatches.
- **An image adapter that polls past litegen's own wait.**

## Constraints

- No commits without the user's go.
- `CARGO_INCREMENTAL=0`, one cargo at a time.
- Ping the peer session before any `openapi.rs` change (none expected).
- `src/mesh/` is untouched.
- No subagents.

## Proof

- **Unit:** rehost classification (no data, non-public URL, 404, 5xx, too
  large, store failure); the extension map; the deadline in a polling adapter
  (bfl and replicate over wiremock: an endless "pending" ends at the
  deadline); 429 retryable in Google, Luma and OpenAI image (wiremock).
- **API and poller:**
  - a mock video that finishes with nothing to keep fails at the first
    client poll and at the first poller tick, refunded, with one webhook;
  - a reaped row sends its webhook (non-retryable poll error, past max age);
  - a BYO-submitted video is polled with the BYO instance.
- **The full gate** (Postgres) and the dashboard suites stay green, and the
  known-gaps file is updated.
