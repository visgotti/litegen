# Multi-file Downloads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The converter (`package=zip`) and 3D generations (`output_packages: true`) deliver OBJ, Collada, USDA and glTF with their sidecars, as a zip.

**Architecture:** `mesh::zip::write_package` turns the library's existing `Package` into a deterministic zip. The converter writes the package for `package=zip`. Delivery adds one `kind: package` asset per requested container whose package has more than one file.

**Tech Stack:** Rust (axum, utoipa; `mesh/zip.rs` with `miniz_oxide`), the TS and Python SDKs, the React dashboard (vitest, Playwright), the Node render harness.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-multi-file-downloads-design.md`

**Status: done on 2026-11-04 by session litegen-05, not committed.** Task 5 (SDKs and
dashboard) was done by litegen-d8, today's frontend/SDK session. Every task below was
carried out. The ledger, `.superpowers/sdd/2026-11-04-mesh-multi-file-downloads/progress.md`,
records each ruling with its reason and its cost. The build differs from the steps in
these places:

- **Order and execution.** The tasks ran inline rather than through subagents, in the
  order 1 → 2 → 4 → 3 → 6. Task 3's delivery edits waited until item 12 released
  `model3d_deliver.rs` (Ruling P3).
- **Task 1.** A fourth test, `package_entry_names_are_canonical_and_unique`, refuses a
  non-canonical path, a duplicate name and a name that differs only in case, and gives a
  non-ASCII name the UTF-8 flag (Ruling S3). The triangle count it compares is the drawn
  count, because USD writes an instanced mesh once per instance.
- **Task 2.**
  - `package` values are trimmed and case-folded, as `fbx_color_space`'s are.
  - The losses header equals the library's package-mode losses exactly. An OBJ package
    still reports the fixture's double-sidedness, which MTL can't state; the spec is
    corrected.
  - `obj.rs`, `dae.rs`, `usd_write.rs` and `glb.rs` now name images so that textures
    with the same name unzip to distinct files (`textures_named_alike_unzip_to_distinct_files`).
- **Task 3.**
  - A package that can't be made is named on its container's mesh asset and logged,
    never silently missing.
  - From the final review, only containers that write sidecars (obj, dae, usda, gltf)
    get packages, so no one-file container is written twice. This is
    `MeshFormat::writes_sidecars()`, tested by
    `only_containers_that_write_sidecars_make_packages`.
  - The OpenAPI asset docs now name `package` assets; litegen-d8 regenerated once more.
- **Shared plumbing** for items 3, 6, 7 and 8: `option_needs_keeping(param, what,
  model)`, the `DeliveryPlan.packages` pattern, and `write_keyed(…, plan)`. The overview's
  rules name them.
- **Proof on the final tree.** The gate ran combined with item 12's, on one frozen tree
  (12:19–12:28 ET, run by litegen-a3 [1a9a54]):
  - `cargo test --lib` with the 3 load-sensitive tests skipped, 2251/0;
  - the 3 alone, 3/3;
  - `--tests` with Postgres, 54 suites, 3088/0;
  - clippy `-D warnings`, clean.

  The harness passed fixtures 570/570 and imports 114/114, with every tool present. Its
  zip check found 0 errors over 686 zips, each of which passes `unzip -t` and
  `zipfile -t`. litegen-d8's Playwright test passed 1/1.
- **Left as minors** (in the ledger):
  - the converter maps a `write_package` refusal to 422 `mesh_malformed`, not 500
    `conversion_failed`; today's writers can't produce one;
  - `option_needs_keeping` answers `validation_error` with `param`, while
    `format_needs_keeping` answers `invalid_request_error` without one;
  - Windows device names (`con`, `nul`, …) in image names aren't renamed;
  - delivery's peak memory with packages hasn't been re-measured.

To commit on the other laptop. This list is from litegen-05 and litegen-d8, and other
items edit several of the same files. Paths are under `litegen-core/` unless marked
(root).

Files with only item 1's changes:
- `src/mesh/zip.rs`: `write_package` and its 6 tests.
- `src/api/handlers/convert.rs`:
  - the `package` parameter (`Packaging`, `PARAMS`, `parse_query`, the
    `convert_in(slots, …)` seam, `convert_bytes`, the zip headers);
  - its OpenAPI (the inline enum, the `application/zip` 200);
  - the zip deadline test.
- `src/proxy/storage.rs`: `"zip" => "application/zip"`.
- `src/mcp/content.rs`: a zip convert reply embeds as `…/<uuid>.<to>.zip`.
- `src/proxy/router.rs`:
  - re-hosting by kind, giving `package_{format}.zip`;
  - the keep-nothing `what` arm;
  - `rehost_stores_a_package_as_a_zip_named_for_its_container`;
  - `output_packages: None` in 3 test literals.
- `src/proxy/poller.rs`: `output_packages: None` in 3 test literals.
- `src/api/openapi.rs`: `packages_are_a_request_field_an_asset_kind_and_a_named_refusal`,
  and `package` in the pinned `Model3dAssetKind` enum.
- `examples/mesh_matrix.rs`: the zip check and the non-ASCII control.
- Tests:
  - `tests/convert_api.rs`: 5 package tests and their helpers;
  - `tests/mcp_api.rs`: `convert_package_zip_embeds_the_zip`;
  - `tests/model3d_api.rs`: 7 tests and their helpers;
  - `tests/model3d_validation.rs` and `tests/live_model3d.rs`: `output_packages: None`,
    one literal each.
- `docs/mcp/tools.md` (root, regenerated).
- `.claude/skills/model3d-provider/SKILL.md` (root): delivery steps 1–2 and the
  keep-nothing sentence.

Shared files, item 1's part only:
- `src/mesh/obj.rs`: the new `unique_file` for texture paths.
- The folded image-name dedupe, a line or two each:
  - `src/mesh/dae.rs`; the rest is item 11's;
  - `src/mesh/usd_write.rs`; the rest is item 11's;
  - `src/mesh/glb.rs`, `write_gltf_scene`'s image naming; the rest is item 12's.
- `src/mesh/mod.rs`: `MeshFormat::writes_sidecars()` and the "One file, or a package"
  paragraph. The lossiness rows are item 11's.
- `src/types/mod.rs`: `Model3dGenerationRequest.output_packages`,
  `Model3dAssetKind::Package`, and the docs of `GenerationAsset.kind` and `Model3dAsset`.
  The rest is the MCP merge's and litegen-41's.
- `src/providers/mod.rs`: `Model3dExtras.output_packages`. `ImageExtras.resolution` is
  litegen-41's, and `SourceInput` is item 12's.
- `src/api/handlers/mod.rs`: in `generate_3d`, `output_packages` into the extras, the
  keep-nothing refusal, `option_needs_keeping()` and the 400 text. The rest is the MCP
  merge's, litegen-3a's, litegen-41's and item 12's.
- `src/proxy/model3d_deliver.rs`:
  - `DeliveryPlan.packages`, `for_request`'s `packages:` line and `manifest()`'s
    sidecars argument;
  - `write_keyed`, `annotate`, the package step, `loss_notes`, `package_zip` and
    `PackageZip`;
  - the 4 tests after "// ── output_packages";
  - two literal fixes in older tests.

  The rekey budget and joint naming are item 12's.
- `src/providers/model3d/mock.rs`: `TEXTURED_MODEL`, `textured_glb()` and its branch.
  The animate and rig work is item 12's.
- `src/providers/model3d/{fal,meshy,tripo}.rs` and `src/providers/config_conformance.rs`:
  `output_packages: None` in test literals only.
- `models/mock.yaml` (root): the `mock/textured-3d` row. The `mock/animate-3d` edit is
  item 12's.
- `README.md` (root): the 3D section's intro line and packages paragraph, and the new
  "Convert a 3D Model" section. The provider and env rows are litegen-41's.
- `docs/superpowers/3d-format-conversion.md` (root): §1's paragraph under the container
  table, and §9's bullet.
- This plan, its spec and the overview (litegen-4e's edits).

litegen-d8's Task 5 (root):
- `dashboard/src/components/`: `model3d-assets.ts` and `model3d-assets.test.ts` (which
  also hold d8's separate fix for dropped `original` assets), `PackageDownloads.tsx`
  (new) and `GenerationOutput.tsx` (which also holds earlier sessions' work);
- `dashboard/src/playground/ResultTile3D.tsx` and `dashboard/e2e/model3d-packages.spec.ts`
  (new);
- the TS SDK:
  - `sdks/typescript/src/client.ts`, `src/index.ts`, `test/convert.test.ts`,
    `test/model3d-types.test.ts`;
  - its `dist`;
- the Python SDK: `sdks/python/litegen/client.py` (both clients), `litegen/types.py`,
  `tests/test_models3d.py`;
- both SDKs' READMEs and CHANGELOGs;
- the regenerated `sdks/openapi.json`, the SDKs' generated trees and
  `apps/landing/public/openapi.json`. `regen-all.sh` rewrites whole generated trees,
  which also carry other sessions' regenerations, so commit them as a whole.

Git ignores Playwright's assets under `dashboard/data/assets/`, `dashboard/dist` and the
pytest caches, so the test runs left nothing to commit.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- The API shape is settled (D1): `package=file|zip`, `output_packages`, `kind: "package"`, and `option_needs_keeping` for a keep-nothing request that sets `output_packages`.
- OpenAPI-visible. Before Task 2, send the frontend/SDK session these exact shapes:
  - converter query `package: "file" | "zip"` (default `file`), with the 200 content type `application/zip` added;
  - request field `output_packages?: boolean`;
  - `Model3dAssetKind` adds `"package"`;
  - error code `option_needs_keeping` (400).

  That session regenerates once the tree builds (`sdks/scripts/regen-all.sh`, then `node apps/landing/scripts/sync-openapi.mjs`).
- `package=file` and requests without `output_packages` stay byte-identical to today.
- Zip entry names come from `Package` paths and must pass `package::normalize_relative`.

## Review Focus

1. A textured scene whose image is WebP or KTX2. Delivery transcodes WebP first; a dropped KTX2 must not leave the MTL or USDA naming a texture file the zip lacks.
2. Two textures with the same name. The package writers must produce distinct paths, and the zip must not hold duplicate entry names (the archive reader refuses duplicates).
3. Non-ASCII texture names. The zip sets the UTF-8 flag. Check that `python3 -m zipfile -t` and `unzip -t` accept them.
4. A `keep_outputs: false` app whose request omits `output_packages`. It must behave exactly as today, with no refusal.
5. A row recorded before `DeliveryPlan.packages` existed, delivered by the poller. Serde's `#[serde(default)]` means no packages, and no parse failure.

---

### Task 1: `zip::write_package`

**Files:** `litegen-core/src/mesh/zip.rs`, `litegen-core/src/mesh/tests.rs`.

**Interfaces:**
- Produces: `pub fn write_package(package: &Package) -> Result<Vec<u8>, MeshError>`. Text extensions (`obj`, `mtl`, `dae`, `usda`, `gltf`) are deflated, everything else is `Stored { align: 1 }`, and the primary comes first.

- [x] Write the three mesh tests from the spec: `write_package_round_trips_through_the_archive_reader`, `a_package_zip_reads_back_as_its_container` and `package_entry_names_never_escape`.
- [x] Run `cargo test --lib mesh::`. Expected: FAIL (`write_package` is undefined).
- [x] Implement with `zip::Writer`.
- [x] Run `cargo test --lib mesh::`. Expected: pass.

### Task 2: The converter's `package` parameter

**Files:** `litegen-core/src/api/handlers/convert.rs` (`PARAMS`, `parse_query`, `convert_bytes`, the response headers, the `#[utoipa::path]` params and 200 content), `litegen-core/src/proxy/storage.rs` (`model3d_content_type`: `"zip" => "application/zip"`), and the convert handler tests.

**Interfaces:**
- Consumes: `zip::write_package`.
- Produces: `ConvertQuery.package: Packaging`, with `enum Packaging { File, Zip }` private to convert.rs.

- [x] Send the frontend/SDK session the shapes (Global Constraints).
- [x] Write the converter tests from the spec: `package_zip_returns_the_whole_package`, `package_file_is_todays_response`, `a_single_file_container_zips_to_one_entry`, `package_values_are_checked`, and the zip case of the send-deadline test.
- [x] Run the convert tests. Expected: FAIL (`package` is an unknown parameter, 400).
- [x] Implement. With `Zip`, `convert_bytes` writes `self_contained: false` and returns the zip as the single body. The filename is `model.<to>.zip`. Losses use the package capabilities.
- [x] Run the convert tests and `cargo test --lib api::openapi`. Expected: pass.

### Task 3: `output_packages` on generations

**Files:**
- `litegen-core/src/types/mod.rs` (`Model3dGenerationRequest.output_packages`, `Model3dAssetKind::Package`)
- `litegen-core/src/providers/mod.rs` (`Model3dExtras.output_packages`, `Model3dFile` kind)
- `litegen-core/src/api/handlers/mod.rs` (extras, the keep-nothing refusal)
- `litegen-core/src/api/middleware/validator.rs` (accept the field on every 3D model)
- `litegen-core/src/proxy/model3d_deliver.rs` (`DeliveryPlan.packages`, writing the packages)
- `litegen-core/src/proxy/router.rs` (`rehost_model3d_files`: stem `package_{format}`, extension `zip`)
- `litegen-core/tests/model3d_api.rs`
- a textured mock row in `models/mock.yaml` and `providers/model3d/mock.rs` (if none returns a textured GLB)

**Interfaces:**
- Produces: `DeliveryPlan.packages: bool` (`#[serde(default)]`).
- Produces: `Model3dFile { kind: Model3dAssetKind::Package, format: "<container>", content_type: "application/zip", … }`.
- Produces: error code `option_needs_keeping`.

- [x] Write the generation tests from the spec: `output_packages_delivers_a_zip_per_multi_file_container`, `packages_follow_the_access_mode`, `a_keep_nothing_request_cannot_ask_for_packages`, `the_poller_delivers_packages`, `the_assets_list_names_packages`. Add one test per Review Focus item 1, 2, 4 and 5: `a_dropped_texture_leaves_no_dangling_reference`, `same_named_textures_get_distinct_entries`, `keep_nothing_without_packages_is_unchanged`, `a_plan_without_packages_parses`.
- [x] Run `cargo test --test model3d_api`. Expected: FAIL (an unknown field, or no package assets).
- [x] Implement:
  - delivery calls `write_scene(.., WriteOptions { self_contained: false, fps: key_fps, fbx_color_space, .. })` once per requested format when `plan.packages`, and emits a package file when `package.files.len() > 1`;
  - the package's `manifest` is measured from the package: `manifest()` gains a sidecars argument, and for the package the primary is read through `MemorySidecars` over its files. Its notes are the package-mode losses;
  - re-hosting names it `package_{format}.zip`.
- [x] Run `cargo test --test model3d_api --test model3d_validation --test model3d_conformance` and `cargo test --lib proxy::`. Expected: pass.

### Task 4: MCP

**Files:** `litegen-core/src/mcp/content.rs` (`file_result` for `convert_3d_model` when the reply is `application/zip`), `docs/mcp/tools.md` (regenerated).

- [x] Write an MCP test: `convert_3d_model` with `package: "zip"` returns an embedded resource whose URI ends `.<to>.zip` and whose mime is `application/zip`.
- [x] Run it. Expected: FAIL (mime is the mesh's).
- [x] Implement, then run `cd litegen-core && cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`.
- [x] Run `cargo test --lib mcp::` and `cargo test --test mcp_api`. Expected: pass, including `every_openapi_operation_is_a_tool_or_excluded` and `tools_reference_is_current`. The tool counts don't change, because no route was added.

### Task 5: SDKs and dashboard (the frontend/SDK session's files)

**Files:**
- `sdks/typescript/src/client.ts` (`ConvertModel3dOptions.package`) and its tests
- `sdks/python/litegen/client.py` (`convert(…, package=None)`) and `sdks/python/tests/test_models3d.py`
- `dashboard/src/components/model3d-assets.ts` (`KINDS`, `packageDownloads`) and its test
- `dashboard/src/components/GenerationOutput.tsx`
- `dashboard/src/playground/ResultTile3D.tsx`
- a Playwright spec

If another session owns these paths, hand it this task with the spec. Otherwise:

- [x] After the regeneration: write the SDK tests and the dashboard vitest from the spec. Run them. Expected: FAIL.
- [x] Implement as pass-throughs: no unzipping in any client.
- [x] Run the TS and Python SDK suites, dashboard vitest, lint, tsc, the build and Playwright. Expected: pass.

### Task 6: Harness, docs and gate

**Files:** `litegen-core/examples/mesh_matrix.rs` (one step: zip each case's package with `zip::write_package`, unpack it with `python3 -I -m zipfile -e` into a scratch folder, compare byte for byte with the case folder), and the docs listed in the spec. The harness already writes and renders packages (`mesh_matrix.rs` package mode; `page/render.js` OBJ+MTL and USDA sidecars), so nothing else in it changes.

- [x] Implement the zip step.
- [x] Run `cargo run --release --example mesh_matrix -- <dir>`, then `node verify.mjs <dir> <report>`. Expected: every zip unpacks byte-equal to its case folder, and 0 failures on the fixture set.
- [x] Run SceneKit, usdrecord, `usdchecker`, `assimp info`, `unzip -t` and `python3 -I -m zipfile -t` over the package outputs. Expected: all accept.
- [x] Negative control: drop the last sidecar in `write_package`. The read-back test and the harness's zip comparison fail. Restore by rewriting.
- [x] Update the docs. Tell litegen-a3 about the matrix cells.
- [x] Run the overview's full gate. Write the ledger.
