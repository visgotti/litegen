# Changelog

Every release of `litegen-sdk`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor release may change behaviour, and says how here.

## [0.1.0] - 2026-10-30

### Added

- Response headers: the `on_response` client option is called with every
  `httpx.Response`, errors included; `response_meta(headers)` reads litegen's
  headers (what lax mode dropped, the rate limit, an idempotent replay,
  `Retry-After`); and `LiteGenAPIError.headers` carries an error response's.
- `idempotency_key` on every billed submit: the same key with the same
  request replays the first answer, billed once.
- `generations.assets(id)` and `generations.download_asset(id, index)`.
- `generations.cancel(id)` returns a `CancelledGeneration`: the row as
  settled (refunded, so `cost_usd` 0) and `vendor_cancelled`.
- `metadata` on every generation and segmentation request, echoed on every
  answer; `logs.list(metadata=...)` and `stats.get(metadata=...)` filter by it.
- `images.segment`, background removal, partial edits, native pixel art,
  output format and transparency; `models3d` rigging, animation, operations,
  placement, multiview and `source_model`; video keyframes, sprite controls
  and source-video operations.
- Vendor errors carry the vendor's own answer (`provider_error`) and whether
  to retry (`retryable`).

### Changed

- A request now waits `DEFAULT_TIMEOUT` (390 s) by default, not 60 s: a
  synchronous call can take three vendor attempts of up to 120 s.
- Polling now waits `JOB_DEFAULT_TIMEOUT` (2 hours 5 minutes) for every job,
  not 300 s (1800 for 3D): the server fails a job still running after 2 hours
  itself. `MODEL3D_DEFAULT_TIMEOUT` equals it.
- The package version is 0.1.0 in `pyproject.toml` as in `__version__`.

### Fixed

- A failed submit is never sent again.
- A poll rides out transient failures instead of raising on the first.

## [0.0.1]

Unpublished first cut, copied from source.
