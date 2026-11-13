# Late images, vendor-held videos and variable prices — design

Round 4 of the known-gaps work (the user, 2026-11-03: "fix the gaps to
ensure our app has full flexibility so users can use it however they need
to"). The design is delegated ("research best practices … build it in the
best way u can"); every decision below is mine.

## Why

- **An image the vendor finishes after litegen's deadline is billed and
  never delivered.** Seven image vendors run jobs (BFL, Replicate, Kling,
  Hunyuan, Leonardo, Luma, Runway). When a job outlasts the vendor timeout
  litegen answers 504 `vendor_timeout`; the vendor still finishes and bills,
  and the image is lost.
- **There is no way to ask for an image asynchronously.** Video and 3D are
  submit-and-poll; images are one blocking call, so a caller with a slow
  model, a queue or a batch has no choice but to hold a connection open.
- **Google Veo's result needs the API key**, so litegen's re-host download
  fails; and a video that only exists as bytes can't be handed out when the
  app keeps nothing, which today fails only after the vendor has billed.
- **Most rows bill every variant at the default price.** 33 of 124 rows have
  `variable_pricing`; `luma/ray-2` charges $0.40 for 5 s at 540p and for 9 s
  at 4k alike.

## Goals

1. **Late delivery.** An image job the vendor accepted is never lost to
   litegen's deadline: the generation stays `processing`, the background
   poller finishes it, stores the image like any other, bills it, and fires
   the key's webhook. `GET /v1/generations/{id}` returns it when done.
2. **Async images on request** (RFC 7240): `Prefer: respond-async` answers
   `202 Accepted` with the generation as soon as the vendor has the job;
   `Prefer: wait=<seconds>` sets how long the call waits before it does
   so. Without either, the call is synchronous as today.
3. **Veo's video is kept**: its adapter downloads the bytes with the key.
4. **A keep-nothing request a model can't serve is refused before the
   vendor is called**, not failed after it bills.
5. **Every row whose vendor's price varies prices its variants.**

## Non-goals

- Late delivery for edits litegen finishes itself (`preserve`: the init
  pasted back outside the edit area). Those need the init image after the
  call; a timed-out one stays a 504.
- Late delivery when the app keeps nothing: there is nowhere to deliver it
  from. A keep-nothing image that times out stays a 504, and
  `Prefer: respond-async` is refused (400) for it.
- Synchronous vendors (OpenAI, Stability, Google, Ideogram, Recraft, fal,
  MiniMax, ByteDance, PixelLab, Bedrock): their one call either answers or
  doesn't; `respond-async` is refused for them (400 `async_unsupported`),
  naming the model.
- New catalog rows (round 5).

## Decisions

### Resumable image jobs

- New trait method `ImageProvider::poll_job(&self, model, job: &str)
  -> Result<ImageJobPoll, ProviderError>`, default `Unsupported`.
  `ImageJobPoll` is `Running`, `Done(GenerationOutput)` or
  `Failed(String)`. The seven polling adapters implement it by sharing one
  poll step with their own `generate` loop, so a job is read the same way
  in both.
- What an adapter records with `extras.vendor_job.record(..)` is what
  `poll_job` needs: the job id, or a small JSON object when the poll needs
  more (BFL's regional `polling_url`; Replicate's `urls.get`).
- `ImageProvider::resumable(&self, model) -> bool` (default false) says
  whether a row's jobs can be resumed; the handler uses it for
  `respond-async`.

### The request

- `Prefer: respond-async` → the adapter's deadline is "now": it submits,
  records the job, and returns at once; the handler answers 202 with
  `{id, object: "image.generation", status: "processing", model,
  created}` and a `Location: /v1/generations/{id}` header.
- `Prefer: wait=N` (1 ≤ N ≤ the vendor timeout) → the deadline is N s; a
  job still running then answers 202 the same way. `Preference-Applied`
  names what was honoured.
- Neither → today's call. A job still running at the vendor timeout is no
  longer only a 504: it answers 504 `vendor_timeout` as before (clients
  that retry on it are unchanged), now carrying `generation_id` and
  `status: "processing"`, and litegen delivers it later.
- Refused up front (400): `respond-async` or `wait` for a model whose jobs
  can't be resumed (`async_unsupported`), with `keep_outputs: false`
  (`async_needs_keeping`), or for a `preserve` edit (`async_unsupported`).

### Delivery

- The handler writes the generation row (`media_type: image`,
  `status: processing`) with `metadata.image_job = {vendor_job, provider,
  extras}`: the `ImageExtras` the result is built with (response format,
  output format, views, directions…), so the poller builds the same result
  the call would have.
- The poller takes `processing` image rows with an `image_job`, builds the
  adapter with the row's BYO credential (as for video), calls `poll_job`:
  - `Done` → the same `build_image_results` the call uses (keeping, access
    mode, signed links), the row `completed` with its assets and artifact,
    the charge settled, the webhook fired;
  - `Failed` → the row `failed` with the vendor's reason, refunded,
    webhooked;
  - `Running` → next tick; a transient `Err` → next tick; the reaper's
    maximum age applies as for video.
- One observer finishes a row: the poller claims it with the same atomic
  update video rows use.

### Billing

- A resumable image call is billed like a video submit from the moment the
  vendor accepts it: the reservation is settled to the row's price and the
  charge recorded on the row (`charged_to`), and a later failure refunds it.
  A sync answer settles as today.

### Veo and keep-nothing

- Google video's poll downloads `video.uri` with the key and returns
  `video_data` (64 MiB cap, a public-address check on a redirect target).
- `VideoProvider::hands_out_links(&self, model) -> bool` (default true):
  false for Google (bytes) and Bedrock (an `s3://` URI). A keep-nothing
  request for such a model is a 400 `keep_outputs_unsupported` before the
  vendor is called.

### Variable pricing

- Every row whose vendor prices by duration, resolution, quality, size,
  mode or audio gets `variable_pricing` rules from the vendor's pricing page
  (quoted in the row's `@see`, re-read 2026-11-03), so the quote, the
  reservation and the charge price the variant asked for. Rows with one
  price stay flat; the report lists each row changed and any price the
  schema can't express.

## Testing

- Unit tests per adapter for `poll_job` (running, done, failed, a passing
  failure), and a conformance test across the seven polling adapters: a job
  that outlasts the deadline is finished by `poll_job` with the same result
  the call would have returned.
- API tests: `respond-async` → 202 then the poller completes the row and
  `GET /v1/generations/{id}` serves the image; a sync timeout → 504 with
  `generation_id`, then delivered; refusals; billing (charged at accept,
  refunded on failure); the webhook.
- Veo download (wiremock), the keep-nothing refusal, and a pricing test per
  changed row (quote = charge for a non-default variant).
- The full cargo gate with Postgres; SDK and dashboard suites (new 202
  answer and fields).

## Left to the builder

- Each adapter's poll step and what it records; which rows' prices vary and
  by what (from their vendors' pricing pages).
