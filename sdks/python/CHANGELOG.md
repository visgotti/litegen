# Changelog

Every release of `litegen-sdk`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor release may change behaviour, and says how here.

## [Unreleased]

### Added

- `package="zip"` on `models3d.convert`: the answer is a zip
  (`content_type` `application/zip`, `filename` `model.<to>.zip`) holding the
  converted file first and whatever goes beside it, such as an OBJ's `.mtl`
  and textures. The SDK returns the zip unopened.
- `ktx2_textures` on `models3d.convert`: `"png"` (the default) re-encodes every
  KTX2 (Basis Universal) texture as PNG, and `"keep"` leaves it as it is when
  `to_format` is `"glb"` or `"gltf"`, which then require `KHR_texture_basisu`.
- `ktx2_textures` on 3D generation requests, and its type
  `Model3dKtx2Textures`: the same choice for delivery. `"keep"` on a request
  that keeps nothing is refused (`option_needs_keeping`).
- `fbx_light_units` and `usd_light_units` on `models3d.convert`: the units FBX
  and USD light strengths are stored in at that end, `"photometric"` (the
  default; glTF's candela and lux) or `"blender"` (so the lights open in
  Blender as bright as the glTF does).
- `fbx_light_units` and `usd_light_units` on 3D generation requests, and
  their type `Model3dLightUnits`: the same choice, for when litegen reads the
  vendor's FBX or USD or writes one for the request.
- `dae_morphs` on `models3d.convert`, when `to_format` is `"dae"`: `"bake"`
  (the default) writes each mesh at its weights and drops the morph targets'
  animation, which every Collada reader opens; `"keep"` writes the targets as
  `<morph>` controllers with their weight animation, which SceneKit (Xcode)
  and Godot read and three.js and assimp refuse.
- `dae_morphs` on 3D generation requests, and its type `Model3dDaeMorphs`:
  the same choice for a Collada litegen writes for the request. `"keep"` on a
  request that keeps nothing is refused (`option_needs_keeping`).
- `"usdc"` in `Model3dFormat`: a USD layer written as a binary crate, for
  `models3d.convert`'s `from_format` and `to_format` and for
  `output_formats`. `"usda"` and `"usdc"` each read either encoding.
- `usdz_root` and `usd_clips` on `models3d.convert`. `usdz_root` (when
  `to_format` is `"usdz"`) writes the package's root layer as `"usda"` text
  (the default) or a `"usdc"` crate, as Apple's own packagers do. `usd_clips`
  (when `to_format` is `"usda"`, `"usdc"` or `"usdz"`) keeps `"all"` clips as
  the variant set `Animation` or the `"first"` alone; by default all for usda
  and usdc, the first for usdz.
- `usdz_root` and `usd_clips` on 3D generation requests, and their types
  `Model3dUsdzRoot` and `Model3dUsdClips`: the same choices for a USD litegen
  writes for the request. `"usdc"` and `"all"` on a request that keeps
  nothing are refused (`option_needs_keeping`).
- `output_packages` on 3D generation requests, and the `"package"` asset kind
  it delivers: one zip per requested container that has files to go beside
  its mesh.
- `FramesSetShape` in `litegen.types`, the values of `frames_set_shape` on a
  video model's `params.aspect_ratio`: whether a `first_frame`, `last_frame`
  or `init` image sets the clip's shape, so `aspect_ratio` is refused (or
  dropped in lax mode) beside one.
- In the generated models: `ModelSchema.litegen_params` (read-only on the
  server: the fields litegen applies to the model itself, in the `ParamSpec`
  shape), `extra_params` (the types of its `extra_allowlist` keys) and
  `status`; `ModelCapabilities.max_request_bytes` (required, as it is on
  every listed model, so the generated class takes it and its `from_dict`
  needs a server that sends it);
  `PromptSpec.required_for_text_to_video`;
  `ParamSpecAspectRatio.frames_set_shape`; `ProviderHealth.modality`
  (required, so the generated `from_dict` needs a server that sends it);
  `GatewaySettings.cache_max_bytes` and `EffectiveCache.max_bytes`;
  `ImageSegmentationRequest.strict`. A schema `models.get` served may be posted
  back to `admin.models.create` or `replace` as it came.

### Changed

- `AnimationRequest.name` is `NotRequired[str | None]`: litegen names a clip
  that leaves it out (`walk` after its preset, else `clip-<n>`).
- What `X-Litegen-Dropped-Params` covers: fields the request has no such name
  for too, one inside an object by its path, on the quotes, `images.segment`
  and the image 202 as well. It lists at most 32 names and ends with a
  `+N more` count past that; `dropped_params` and `response_meta` keep that
  entry.
- `LiteGenAPIError.retryable` comes with every router error, such as a 503
  `circuit_open` (every deployment of a model route cooling down, with
  `retry_after`), not only a vendor's. New codes callers may see:
  `circuit_open`, `cache_clear_incomplete` (`cache.clear`, 503),
  `request_unsupported` and `prompt_required` (both `LiteGenValidationError`),
  `provider_not_configured`, `range_not_satisfiable` (416).

### Removed

- The `additional_properties` bag of nine generated models, whose schemas
  now refuse unknown keys (`additionalProperties: false`): `ModelSchema`,
  `CapabilityModelPricing`, `ModelCapabilityFlags`, `PromptSpec`,
  `RefInputSpec` and `RefRoleSpec` (also re-exported at `litegen.*`), and
  `NativeOrientation`, `AnimationPresetSpec` and `TextToMotionSpec` in
  `litegen._generated.models`. They lose `additional_properties`,
  `additional_keys`, `obj[key]`, `obj[key] = value`, `del obj[key]` and
  `key in obj`; `from_dict` drops a key the schema does not name instead of
  keeping it, and `to_dict` sends declared fields only.

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
