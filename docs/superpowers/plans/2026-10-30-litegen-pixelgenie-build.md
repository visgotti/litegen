# litegen: PixelGenie requirements — build plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline) with superpowers:test-driven-development on every step.

**Goal:** build what [`2026-10-30-litegen-3d-and-partial-edit-apis.md`](2026-10-30-litegen-3d-and-partial-edit-apis.md) (the **3D doc**) and the still-standing parts of [`2026-10-29-litegen-requirements.md`](2026-10-29-litegen-requirements.md) (the **L doc**: L1–L4, L7–L10, L13–L19) ask of litegen, P0 first across both, then P1, then P2; then verify every provider and model against its vendor's current docs, and the dashboard against the API.

**Spec of record:** the 3D doc. Its §2 rules decide every API shape; where the L doc and the 3D doc disagree, the 3D doc wins. The L doc's code citations come from the stale `e0676a3` tree and are re-checked before use.

**Base:** `origin/3d-models` @ `9f1349e` (local `3d-models` equal to it at start).

## Global constraints

- **Each item ships in the same change with:** its deterministic mock (`models/mock.yaml` + `providers/*/mock.rs`), a conformance test, its rows in `docs/superpowers/param-mapping-matrix.md`, and `sdks/openapi.json` + the TS `schema.d.ts` + the Python `_generated` package regenerated (`./sdks/scripts/regen-all.sh`), plus the hand-written SDK surfaces those regenerations break (`sdks/python/litegen/types.py` drift tests, `contract-allowlist.json`).
- **§2 rule 5:** every new request field is a `KNOWN_PARAMS` entry with a `ParamSpec`, appears as a `supports_*` flag in `ModelCapabilities`, is refused in strict mode and dropped + reported (`X-Litegen-Dropped-Params`) in lax mode where a model does not declare it.
- **§2 rule 6:** anything that can be refused is refused before the billed submit (before `reserve_quota`).
- **Vendor facts:** every vendor field is cited in code (`@see` with a verbatim quote) and in `docs_url`. A doc row marked **(unverified)** is re-checked against the vendor's docs before code relies on it. No vendor keys exist in this environment, so live billed proofs are not possible here: cells that need one stay 🟡 in the matrix, never ✅.
- **SDKs stay thin** (memory: thin-sdks): request/response wrapping and types only; anything a client needs to know comes over the wire.
- **Git:** local commits on `3d-models` only, each with an explicit pathspec (`git commit -m … -- <paths>`), never leaving anything staged. No push. No Claude/Anthropic attribution anywhere.
- **Build hygiene:** `CARGO_INCREMENTAL=0`; disk is ~98% full, so keep one `target/`; no concurrent cargo with any peer session.
- **Test command (Rust):** `cd litegen-core && CARGO_INCREMENTAL=0 cargo test` — the whole suite is the gate for every task (per-step runs may target one test binary).

## Rulings made while planning

Each is also in the ledger. Format: decision — why — cost if wrong.

- P-R1. `task` is one Rust enum `ModelTask` covering every modality's tasks, with `ModelTask::allowed_for(media_type)` checked by the loader — one OpenAPI enum is easier for both SDKs than three — a client sees values that are invalid for its modality in the type.
- P-R2. Generation metadata becomes merge-on-write (RFC 7396 `json_patch` in SQLite, `jsonb ||` in Postgres) instead of replace — the row now carries the request, the source lineage, stage claims, the delivery plan and idempotency data, and every replace-write was erasing what the others wrote — a merge that keeps a stale key alive where a replace used to clear it.
- P-R3. Idempotency is one middleware on every billed POST: same key + same body within 24 h replays the stored response (`Idempotent-Replayed: true`, nothing billed); same key + different body is 409 `idempotency_key_reused`; a concurrent duplicate is 409 `idempotency_request_in_progress`. Pre-execution refusals (validation, auth, 404 model, rate limit, capacity) are not stored, so a corrected request may reuse the key (Stripe's rule) — a client that expected the error itself to replay.
- P-R4. Stage two of a vendor flow starts only after an atomic claim on the row (`metadata.stage_claims.<stage>`); a claim whose start was interrupted (no stage context recorded within 10 min) fails the generation rather than starting the stage again — litegen never re-sends a billed call it cannot prove did not land — a stuck job fails where a retry might have rescued it.
- P-R5. `source_model` errors: `source_not_found` is 404; a source that exists but is not `completed` is a new `source_not_ready` (409); a missing `format` on url/base64 is `source_format_required` (400); the rest of §3A's codes are 422 — the doc's list covers neither case — two extra codes for PixelGenie to map.
- P-R6. Local storage keeps assets on disk under `image_storage.local_dir` (default `./data/assets`) for `image_storage.retention_days` (default 7; values below 7 are raised to 7) — §3A's "at least 7 days" — disk use grows with traffic for a week.
- P-R7. A vendor mesh litegen cannot read (Draco, meshopt) or whose required extension it cannot remove fails delivery with a named error instead of being passed through — rule 3 says every delivered glTF has an empty `extensionsRequired` and every delivered mesh is inspected — `mock/partial-3d` asked for `[glb]` alone now fails where it used to complete; its documented `[glb, obj]` / `[glb, fbx]` behaviour is unchanged.
- P-R8. A KTX2 texture litegen cannot transcode is dropped from the normalised file and reported as a loss; the vendor's file is still delivered as the `original` asset — glTF core allows only PNG/JPEG and there is no Basis transcoder in-process — a textured vendor file loses that texture in the normalised copy.
- P-R9. The vendor's own mask convention is declared per model on the mask role (`ref_inputs.roles.mask.convention`: `white_edits` | `black_edits` | `transparent_edits`); the catalog then serves `mask_convention: "white_edits"` for every model that takes a mask, because litegen converts — keeps the vendor fact next to the role it describes — one more yaml field per mask model.

## Order

P0 (Tasks 1–14) → P1 (Tasks 15–26) → P2 (Tasks 27–36) → vendor-audit fixes (Tasks 37–42) → dashboard (Task 43) → whole-branch verification (Task 44).

---

## Task 0: remove the models past their shutdown date (pre-flight; part of L14)

The baseline suite fails `catalog_conformance::models_past_their_announced_shutdown_are_removed`: `openai/gpt-image-1` (shut 2026-10-23) and `google/veo-3.1-generate-preview`, `veo-3.1-fast-generate-preview`, `veo-3.1-lite-generate-preview` (shut 2026-10-22) are still served. Every later task gates on a green suite, so this goes first.

**Files:** `models/openai.yaml`, `models/google.yaml`, `scripts/model-drift/providers/{openai,google}.mjs`, `litegen-core/tests/catalog_conformance.rs`, `litegen-core/tests/fixtures/retired-models.yaml`, plus any test, dashboard fixture or doc that names the four ids.

- [ ] The failing test is the RED. Remove the rows, move the ids from `SCHEDULED_SHUTDOWNS` to the retired list the test names, and fix whatever else referenced them.
- **Test:** whole suite. **Expected:** green.

## Task 1: `task` on every catalog model (3D doc §2 rule 1)

**Files:** `litegen-core/src/capabilities/schema.rs`, `capabilities/loader.rs`, `capabilities/loader_tests.rs`, `types/mod.rs` (`ModelCapabilities`), `api/handlers/mod.rs` (`project_model_info`), `tests/catalog_conformance.rs`, `docs/superpowers/param-mapping-matrix.md`, SDK regen.

**Interfaces — produces:** `capabilities::ModelTask` (`generate` | `rig` | `animate` | `texture` | `remesh` | `segment` | `multiview` | `background_swap` | `extend` | `retake` | `video_edit`), `ModelTask::allowed_for(MediaType) -> bool`, `ModelSchema.task` (serde default `generate`), `ModelCapabilities.task`.

- [ ] RED: loader tests — a 3D yaml with `task: rig` loads; `task: extend` on an image model is refused; `image_to_3d: true` on a `task: rig` model is refused; an omitted task reads as `generate`.
- [ ] RED: conformance — every shipped model's task is allowed for its media type; `*_to_3d` flags only on 3D `generate`; `GET /v1/models` and `GET /v1/models/{id}` carry `task`.
- [ ] GREEN, then regen OpenAPI + SDKs; matrix gains a "Catalog fields" row for `task`.
- **Test:** whole suite. **Expected:** green.

## Task 2: durable local asset storage (3D doc §3A "Storage must be durable", §3G, L3's storage half)

**Files:** `proxy/storage.rs`, `config/mod.rs` (`ImageStorageConfig.local_dir`, `retention_days`), `api/handlers/mod.rs` (asset route), `main.rs` (sweeper), tests in `storage.rs` + `tests/model3d_api.rs`.

**Produces:** `ImageStorage::get(key) -> Result<Option<(Bytes, String)>, ImageStoreError>` (local disk and S3), `LocalDiskStorage { root, public_base_url }`, `spawn_retention_sweeper`.

- [ ] RED: bytes put through one store instance are served by a new instance over the same dir (restart); a key containing `..` or an absolute path is refused; the sweeper deletes only files older than retention; `retention_days: 1` is raised to 7.
- [ ] GREEN: the asset route reads from disk; the in-memory map is gone.
- **Test:** whole suite.

## Task 3: merge-on-write generation metadata and stage claims (P-R2, P-R4; 3D doc §3G)

**Files:** `db/trait_def.rs`, `db/sqlite.rs`, `db/postgres.rs`, `db/sqlite_tests.rs`, callers in `api/handlers/mod.rs` and `proxy/poller.rs`.

**Produces:** `DatabaseStore::merge_generation_metadata_if_active(id, patch)`, `update_generation_if_active` merging its `metadata`, `DatabaseStore::claim_generation_stage(id, stage, claimant) -> bool`.

- [ ] RED (sqlite): a terminal write keeps `requested_formats` and `request` written at submit; `claim_generation_stage` succeeds once, fails for the second claimant and on a terminal row.
- [ ] GREEN for SQLite and Postgres (Postgres SQL unexercised here without a server — say so in the ledger).

## Task 4: stages after submit (3D doc §3G "Stages after submit")

**Files:** `providers/mod.rs` (`Model3dGenerationPollResult.stage_context`, `.next_stage`; `Model3dProvider::start_stage`), `proxy/router.rs`, `proxy/poller.rs`, tests in both.

**Consumes:** Task 3's claim and merge.

- [ ] RED: a scripted two-stage provider — the client's poll and the poller race on stage one's completion and `start_stage` is called exactly once; the new stage context is persisted and a poller rebuilding the handle from the row sees it; an interrupted claim older than 10 minutes fails the generation.
- [ ] GREEN.

## Task 5: `Idempotency-Key` on every billed POST (L1, 3D doc §3G) — P-R3

**Files:** new `api/middleware/idempotency.rs`, migrations `sqlite/20240101000016_idempotency.sql` + `postgres/…`, `db/*`, `api/handlers/mod.rs` (route layering), `proxy/router.rs` (no retry after an image vendor timeout; 504 `vendor_timeout` carrying `vendor_job_id` when known), `config/mod.rs` (`server.vendor_timeout_secs`), `providers/image/mock.rs` (`mock/slow-image`), `models/mock.yaml`, `api/openapi.rs`, SDKs (`idempotencyKey` / `idempotency_key`).

- [ ] RED: two POSTs with one key make one vendor call, one log row, one quota charge, and return the same body/id (`Idempotent-Replayed: true`); a different body is 409 `idempotency_key_reused`; a concurrent duplicate is 409 `idempotency_request_in_progress`; a validation error does not consume the key; replay works across all three families; `mock/slow-image` times out once and shows exactly one vendor call and a 504.
- [ ] GREEN; OpenAPI documents the header and the 409s on the three operations; SDK option passes the header through.

## Task 6: the job row exists when the submit returns, with the request on it (L2)

**Files:** `api/handlers/mod.rs` (`generate_video`, `generate_3d`), DB.

- [ ] RED: an immediate `GET /v1/generations/{id}` after a video submit answers 200; a failing `insert_generation` makes the submit fail (5xx, `generation_record_failed`) for video and 3D; `metadata.request` holds prompt, params and reference hashes (sha256 + byte length, never bytes).
- [ ] GREEN.

## Task 7: results litegen keeps and serves (L3)

**Files:** `proxy/router.rs`, `proxy/poller.rs`, `api/handlers/mod.rs` (`get_video_status` reads the row once terminal and never forgets on read; image rows), new routes `GET /v1/generations/{id}/assets` and `GET /v1/generations/{id}/assets/{n}`, `types/mod.rs` (`GenerationAsset`), SDKs (`generations.assets`).

- [ ] RED: a completed video is re-hosted and downloads with only a litegen key after the vendor URL is gone; reading a finished video twice gives the same answer; image results get a `completed` row with their assets; the assets route lists `{kind, format, url, size_bytes, sha256, width?, height?, duration_s?, fps?}` and streams bytes to a key that may read the generation, 404 to another org.
- [ ] GREEN; OpenAPI + SDKs.

## Task 8: `source_model` and references by generation id (3D doc §3A, §2 rule 2) — P-R5

**Files:** `types/mod.rs` (`SourceRef`, `RefImageKind::Generation`), `capabilities/schema.rs` (`ParamSpec::SourceModel`), `capabilities/loader.rs` (non-generate 3D tasks require it), `api/middleware/validator.rs`, new `proxy/model3d_source.rs`, `providers/mod.rs` (`Model3dExtras.source`, `Model3dGenerationHandle.source_delivery`), `proxy/materializer.rs` (generation refs for images), `providers/model3d/mock.rs` + `models/mock.yaml` (`mock/rig-3d`, basic), SDKs.

- [ ] RED: validator — strict refuses / lax drops `source_model` on a model without it; a rig model without one is `source_model_required`; url/base64 without `format` is `source_format_required`.
- [ ] RED: resolution — another org's id is `source_not_found` (404); a pending source is `source_not_ready`; a 4-triangle cube against `max_faces: 2` is `source_too_many_faces`; `requires_skin` on the cube is `source_not_rigged`; `requires_texture` on an untextured mesh is `source_not_textured`; a source above `max_bytes` is `source_too_large`; an fbx source to a glb-only model arrives converted to glb; none of these bill.
- [ ] RED: lineage — the operation's row records `metadata.source = {generation_id, sha256, format, delivery}`; a same-vendor, same-account source passes the vendor task id (`delivery: vendor_task`).
- [ ] GREEN; matrix "3D operations" section; OpenAPI + SDKs.

## Task 9: normalised delivery and the mesh manifest (3D doc §3D, §2 rule 3) — P-R7, P-R8

**Files:** new `mesh/inspect.rs` (manifest facts), `mesh/humanoid.rs` (VRM map tables), `mesh/normalize.rs` (root rotation, unique names, joint and clip renames, root-motion removal, texture transcode), new `proxy/model3d_deliver.rs` (the ONE post-processing step both observers call, replacing their separate synthesis calls), `types/mod.rs` (`Model3dAsset.sha256`, `.scene`, `Model3dAssetKind::Original`, `SceneManifest`), `capabilities/schema.rs` (`native_orientation`), `Cargo.toml` (`image` features `jpeg`, `webp`), `mock/turned-3d`.

- [ ] RED: the mock cube's manifest is exact (bounds, 12 triangles, 24 vertices, 1 uv set, materials, `extensions_used: []`); `mock/turned-3d` is delivered facing +Z with `oriented_by: litegen` plus its `original`; a model without proven orientation reports `front: "unverified"`; a WebP-textured GLB is delivered with an empty `extensionsRequired` and its original; duplicate joint names get numeric suffixes; every asset's `sha256` matches its bytes; both observers produce identical assets.
- [ ] GREEN; OpenAPI + SDKs; matrix rows.

## Task 10: rigging (3D doc §3B)

**Files:** `capabilities/schema.rs` (`skeleton`, `joint_naming`, `height_m` in `KNOWN_PARAMS`), `types/mod.rs`, validator, `providers/mod.rs` (`Model3dProvider::preflight`), handler (preflight before quota → 422 `not_riggable`), `mock/rig-3d` (skeleton, `joint_naming: vrm` renames, a flat source is not riggable), real adapters per the research report: first-party `tripo` (rig-check → rig, `spec` from `joint_naming`, task-id chaining) and fal `fal-ai/meshy/rigging`, wiremock tests, catalog rows with `docs_url`.

- [ ] RED: mock rig output's manifest `skin` matches §3I exactly, including the `humanoid` map; `joint_naming: vrm` renames joints; a non-riggable source is a 422 `not_riggable` and bills nothing; Tripo's outbound `rig_type`/`spec`/input task id; the fal endpoint's outbound fields.
- [ ] GREEN; matrix; OpenAPI + SDKs.

## Task 11: animation (3D doc §3C)

**Files:** `capabilities/schema.rs` (`ParamSpec::AnimationList`), `types/mod.rs` (`AnimationRequest`, `animations`, `fps` on 3D), validator, delivery (clip rename in request order, litegen in-place, fps resample), `mock/animate-3d`, real adapters (Tripo retarget, Meshy animations) with wiremock tests.

- [ ] RED: validation of names/presets/prompts/durations/max_items; mock output — one 1 s LINEAR clip per name in request order, in-place root travel 0 vs 1 m +Z, fps honoured, unrigged source `source_not_rigged`; manifest clip facts measured from keys; Tripo `animate_in_place` and preset ids; Meshy `action_ids` and `change_fps`.
- [ ] GREEN; matrix; OpenAPI + SDKs.

## Task 12: the §3J chain, end to end

**Files:** `litegen-core/tests/model3d_chain.rs`, `scripts/mesh-verify` (a node check with `@gltf-transform/core`).

- [ ] RED→GREEN: base64 still → image-to-3D → rig (by generation id) → animate (`walk`, `idle`, in place); the final GLB reads in `@gltf-transform/core` with exactly two animations named `walk` and `idle`; manifest has a `humanoid` map, both clips `in_place: true` with root travel < 1 cm, front +Z; every step re-sent with its `Idempotency-Key` returns the same id and bills nothing; a restart (new app over the same DB file and asset dir) between steps keeps the chain working.

## Task 13: partial image edits (3D doc §4, P0 part) — P-R9

**Files:** `types/mod.rs` (`Region`, `region`, `mask_grow_px`), `capabilities/schema.rs` (`ParamSpec::Region`, mask role `convention`, `preserves_unmasked`), new `proxy/image_edit.rs` (decode, validate, mask ∩ region, dilation, conversion, box mapping with window enlargement), image handler (before quota), adapters: OpenAI (white → transparent), Stability (white, explicit `grow_mask`), Ideogram 4.5 Precise Edit (new endpoint: box `context_window` + black mask), mocks `mock/inpainting-image` (fills the edit area `#ffcd75`, everything else byte-identical, honours `mask_grow_px`) and `mock/region-image` (box input, 256 px minimum window).

- [ ] RED: errors before billing — `init_required`, `mask_size_mismatch`, `mask_ambiguous` (any alpha < 255), `region_out_of_bounds`, `region_too_small`; one white-edits mask arrives at OpenAI as alpha-0-where-white and at Stability unchanged with `grow_mask=0`; region on Ideogram 4.5 sends the exact `context_window`; region on a mask-only model arrives as a drawn mask; the mocks keep every pixel outside the edit area byte-identical.
- [ ] GREEN; catalog flags `supports_inpainting`, `supports_region_edit`, `region_input`, `preserves_unmasked`, `mask_convention`; matrix; OpenAPI + SDKs.

## Task 14: base64 references for URL-only vendors (L4)

**Files:** `proxy/materializer.rs`, `capabilities/schema.rs` (per-model `ref_inputs.data_uri: false`), new short-lived `GET /v1/refs/{token}` serving route, catalog rows per the audit's findings on which URL-only vendors accept `data:` URIs.

- [ ] RED: a base64 reference to a model whose vendor refuses data URIs is published at an unguessable HTTPS URL that expires within the hour and lives until the job ends; `fal/trellis` still receives a data URI.
- [ ] GREEN.

---

## P1

Each P1/P2 task below is a brief; it is expanded in the ledger (as rulings) when started.

## Task 15: multiview images and multiview-to-3D (3D doc §3E)
`ModelTask::Multiview`, `views` string-array param, `ImageResult.view` + `yaw_deg`, responses returning every image (L13's `n`), `mock/multiview-image` (one tinted image per view), the yaw convention, `view-<name>` roles on 3D models, at least one real vendor per half (Tripo on fal; Hunyuan 3.1 Pro multiview on fal).

## Task 16: timed keyframes (3D doc §5, P1)
`ReferenceImage.time_s`, `keyframe` role, catalog `supports_keyframes`, `max_keyframes`, `keyframe_timing`; vendor conversions (Luma frame index at 24 fps, PixVerse durations, Runway fractions); `mock/keyframe-video` frame-at-time equals keyframe.

## Task 17: prices that match the bill (3D doc §3G, L14)
`variable_pricing` applied to quote, reservation and charge together (Rodin HighPack ×3, textured Hunyuan ×3, per-second video, per-megapixel FLUX, per-operation credits).

## Task 18: a truthful `is_available` (3D doc §3G, L14)
`false` when the caller's org has neither its own credential nor a platform default for the provider.

## Task 19: request-time checks (L13)
400 `capability_unsupported`; `n` returns all images or is refused; unknown model ids are errors in every adapter (no substitution); sizes are refused, never snapped.

## Task 20: a truthful catalog (L14)
`status` (`active` | `deprecated` | `retired`), `deprecated_at`, `retires_at`, `replaced_by`; retired models no longer served; Veo 3.1 previews removed and replacements added per the audit; `max_reference_images` (keeping `max_images`); real `max_duration_seconds`; `catalog_version`.

## Task 21: vendor errors that say what happened (L15)
Vendor 4xx → litegen 4xx `vendor_rejected` with `provider_error: {status, code, message}`; 5xx/timeout → 502/504 with `retryable`; `code` always a string.

## Task 22: output format and transparency as real fields (L9)
`output_format` (`png` | `webp` | `jpeg`) and `background` (`transparent` | `opaque`) on images; catalog `output_formats`, `supports_transparent_background`, `lossy_only`; `content_type` sniffed from bytes.

## Task 23: video controls for sprites (L10)
`camera` (`fixed` | `auto`), `audio`, `loop`; catalog flags; discrete `duration_seconds` values; finished jobs report `{duration_s, fps, width, height, has_audio}`.

## Task 24: promptable segmentation (L7)
`POST /v1/images/segmentations` with point/box/text prompts → masks; fal SAM 3; mock (box → box mask, point → 32×32 square).

## Task 25: background removal (L8)
Background-removal models on the edit path; `supports_background_removal`; mock (`#ff00ff` → transparent); fal Bria RMBG / BiRefNet, Recraft, Stability.

## Task 26: multiview vendor wiring leftovers and P1 matrix sweep

---

## P2

## Task 27: texture, remesh, segment operations (3D doc §3F) — mocks `mock/texture-3d`, `mock/remesh-3d`, `mock/segment-3d`, manifest `parts`.
## Task 28: opt-in placement `ground`, `center`, `height_m` (3D doc §3D).
## Task 29: litegen joint renaming for vendors whose names it can map (3D doc §3B).
## Task 30: `expand` and `supports_expand` (3D doc §4 P2), `mock/expand-image`.
## Task 31: `layout` (per-box prompts) and `mask_prompt` (3D doc §4 P2).
## Task 32: `preserve_unmasked` and the `background_swap` task (3D doc §4 P2).
## Task 33: `source_video`, `extend`, `retake`, `video_edit` (3D doc §5 P2), mocks `mock/extend-video`, `mock/retake-video`.
## Task 34: pixel-art vendors PixelLab and Retro Diffusion (L16).
## Task 35: stored request metadata and spend per tag (L17); cancel at the vendor and refund quota (L18).
## Task 36: SDK versioning, response headers, timeouts that match the server (L19).

## Vendor audit fixes

## Task 37: fal catalog and adapters per audit.
## Task 38: google / openai / bedrock per audit.
## Task 39: bytedance / minimax / vidu per audit.
## Task 40: kling / pixverse / hunyuan / luma per audit.
## Task 41: runway / leonardo / replicate per audit.
## Task 42: bfl / ideogram / recraft / stability per audit; drift script sources repaired so `check.mjs` exits 0 or 1, never 2.

## Task 43: dashboard reflects the API
Playground controls for every new param kind (source picker by generation id, skeleton, joint naming, animations list, region box, mask grow, views, keyframe times, output format, background, camera/audio/loop); model detail shows `task` and the new catalog fields; the 3D result tile shows the manifest, clips and the `original`; vitest + Playwright e2e.

## Task 44: whole-branch verification
Every requirement in both docs checked off against code and a test that exercised it; the final review per executing-plans.
