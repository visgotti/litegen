# Changelog

Every release of `@litegen/sdk`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor release may change behaviour, and says how here.

## [Unreleased]

### Added

- `package: "zip"` on `models3d.convert`: the answer is a zip
  (`application/zip`, `model.<to>.zip`) holding the converted file first and
  whatever goes beside it, such as an OBJ's `.mtl` and textures. The SDK hands
  the zip back unopened.
- `ktx2Textures` on `models3d.convert`: `"png"` (the default) re-encodes every
  KTX2 (Basis Universal) texture as PNG, and `"keep"` leaves it as it is when
  `to` is `glb` or `gltf`, which then require `KHR_texture_basisu`.
- `ktx2_textures` on 3D generation requests, and its type
  `Model3dKtx2Textures`: the same choice for delivery. `"keep"` with
  `keepOutputs: false` is refused (`option_needs_keeping`).
- `fbxLightUnits` and `usdLightUnits` on `models3d.convert`: the units FBX
  and USD light strengths are stored in at that end, `"photometric"` (the
  default; glTF's candela and lux) or `"blender"` (so the lights open in
  Blender as bright as the glTF does).
- `fbx_light_units` and `usd_light_units` on 3D generation requests, and
  their type `Model3dLightUnits`: the same choice, for when litegen reads the
  vendor's FBX or USD or writes one for the request.
- `daeMorphs` on `models3d.convert`, when `to` is `dae`: `"bake"` (the
  default) writes each mesh at its weights and drops the morph targets'
  animation, which every Collada reader opens; `"keep"` writes the targets as
  `<morph>` controllers with their weight animation, which SceneKit (Xcode)
  and Godot read and three.js and assimp refuse.
- `dae_morphs` on 3D generation requests, and its type `Model3dDaeMorphs`:
  the same choice for a Collada litegen writes for the request. `"keep"` with
  `keepOutputs: false` is refused (`option_needs_keeping`).
- `"usdc"` in `Model3dFormat`: a USD layer written as a binary crate, for
  `models3d.convert`'s `from` and `to` and for `output_formats`. `"usda"` and
  `"usdc"` each read either encoding.
- `usdzRoot` and `usdClips` on `models3d.convert`. `usdzRoot` (when `to` is
  `usdz`) writes the package's root layer as `"usda"` text (the default) or a
  `"usdc"` crate, as Apple's own packagers do. `usdClips` (when `to` is
  `usda`, `usdc` or `usdz`) keeps `"all"` clips as the variant set
  `Animation` or the `"first"` alone; by default all for usda and usdc, the
  first for usdz.
- `usdz_root` and `usd_clips` on 3D generation requests, and their types
  `Model3dUsdzRoot` and `Model3dUsdClips`: the same choices for a USD litegen
  writes for the request. `"usdc"` and `"all"` with `keepOutputs: false` are
  refused (`option_needs_keeping`).
- `output_packages` on 3D generation requests, and the `"package"` asset kind
  it delivers: one zip per requested container that has files to go beside
  its mesh.

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
