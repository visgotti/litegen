# Changelog

Every release of `@litegen/sdk`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor release may change behaviour, and says how here.

## [0.1.0] - 2026-10-30

### Added

- `VERSION`, this package's version.
- Response headers: the `onResponse` client option sees every response,
  errors included; `readResponseMeta(headers)` reads litegen's headers (what
  lax mode dropped, the rate limit, an idempotent replay, `Retry-After`); and
  `LiteGenAPIError.headers` carries an error response's.
- `idempotencyKey` on every billed submit (`images.generate`,
  `videos.generate`, `models3d.generate`): the same key with the same request
  replays the first answer, billed once.
- `generations.assets(id)` and `generations.downloadAsset(id, index)`: every
  kept result, read with a litegen key.
- `generations.cancel(id)` resolves to a `CancelledGeneration`: the row as
  settled (refunded, so `cost_usd` 0) and `vendor_cancelled`.
- `metadata` on every generation and segmentation request, echoed on every
  answer; `logs.list({ metadata })` and `stats.get({ metadata })` filter by it.
- `images.segment`, background removal, partial edits (`region`, masks,
  `mask_prompt`, `layout`, `expand`, `preserve_unmasked`), native pixel art
  (`palette`, `directions`), output format and transparency.
- `models3d`: rigging, animation, texture, remesh, segment, placement,
  multiview-to-3D, `source_model`, and the measured `scene` manifest.
- Video: timed keyframes, sprite controls (`camera`, `audio`, `loop`),
  `extend`, `retake` and `video_edit` on a `source_video`, with `segments`.
- Vendor errors carry the vendor's own answer (`providerError`) and whether to
  retry (`retryable`); every error `code` is a string.

### Changed

- A request now waits `DEFAULT_TIMEOUT_MS` (6.5 minutes) by default, not
  60 s: a synchronous call can take three vendor attempts of up to 120 s.
- Polling now waits `JOB_DEFAULT_TIMEOUT_MS` (2 hours 5 minutes) for every
  job, not 5 minutes (30 for 3D): the server fails a job still running after
  2 hours itself. `MODEL3D_DEFAULT_TIMEOUT_MS` equals it.
- `stats.get` takes `(opts?, signal?)`; `stats.get(signal)` still works.

### Fixed

- An already-aborted signal no longer sends the request.
- A poll rides out transient failures (a 429, a 5xx, a dropped connection)
  instead of throwing on the first.

## [0.0.1]

Unpublished first cut, copied from source.
