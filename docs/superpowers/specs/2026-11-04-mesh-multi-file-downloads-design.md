# Multi-file downloads (OBJ, Collada, USDA and glTF packages) — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: M.
OpenAPI-visible: the converter gains a parameter, and 3D requests and assets gain a field
and a kind.

**Done on 2026-11-04** by session litegen-05, with Task 5 by litegen-d8. Not committed.
The ledger is `.superpowers/sdd/2026-11-04-mesh-multi-file-downloads/progress.md`. One
correction came from the build (Task 2's ruling), and the four passages it touches are
fixed below. A package reports no textures lost, but it can still report a limit of its
own container. The textured fixture's panel is double-sided, and MTL can't state that in
either mode, so an OBJ package reports `materials: double-sidedness of 1 material(s)`.

## Why

The library already writes every multi-file container as a package
(`WriteOptions { self_contained: false }` → `Package`, primary file first):

- `.obj` + `.mtl` + textures;
- `.dae` + `textures/*`;
- `.usda` + `textures/*`;
- `.gltf` + `.bin` + images.

Both production paths throw that away and write one file:

- the converter (`api/handlers/convert.rs`, `self_contained: true`, "The output is
  always one file");
- generation delivery (`proxy/model3d_deliver.rs`, `WriteOptions { ..Default }`, then
  `package.files.into_iter().next()`).

So an OBJ arrives with its materials baked into vertex colours and no textures. Collada
and USDA arrive with no textures, and the asset's notes report the loss.
`docs/superpowers/3d-format-conversion.md` §9 gives the reason: sidecar references
"need rehosting to keep relative names together". The matrix page marks OBJ, Collada and
USDA textures "package only".

## Where it stands today

- `mesh::Package { files: Vec<PackageFile { path, bytes }> }`, built by
  `obj::write_scene`, `dae::write_scene`, `usd::write_usda` and `glb::write_gltf_scene`
  with `self_contained: false`.
- `mesh::zip::Writer` writes deterministic archives (fixed DOS time, `Stored { align }`
  or `Deflated` at level 3) for USDZ and 3MF.
- Re-hosting (`proxy::router::rehost_model3d_files`) stores one object per
  `Model3dFile` at `{prefix}/{generation}/{name}.{ext}` and hands out its URL in the
  app's access mode. That URL is the stored URL (`public`), a per-file signed link
  (`signed`), or `GET /v1/generations/{id}/assets/{index}` (`private`).

## Decisions

### Zip packages, not several URLs (D1, settled with the user: zip, opt-in)

A relative reference inside a delivered file (`mtllib model.mtl`, `textures/base.png`)
resolves only when the sidecar sits at the same URL prefix and opens with the same
credentials. That holds in `public` mode only:

- `signed`: every file has its own signature in its query string.
- `private`: files are addressed by index (`…/assets/{index}`), not by name.

A zip of the package works in every mode, needs one download per container, and keeps
the SDKs thin: they download bytes, as today.

### The converter: `package=file|zip`

- New query parameter `package`, values `file` (the default, today's behaviour,
  byte-identical) and `zip`. It joins `PARAMS`. A repeat is `param_repeated`, another
  value `param_invalid`.
- With `zip`:
  - the scene is written with `self_contained: false`;
  - the body is a zip of the package's files at their package paths, primary first;
  - `Content-Type: application/zip`, `Content-Disposition: attachment;
    filename="model.<to>.zip"`;
  - the losses header is computed in package mode (`to.capabilities(WriteOptions {
    self_contained: false, .. })`), so an OBJ package reports no textures lost and no
    materials baked into vertex colours. A limit of MTL itself, such as its missing
    double-sided flag, is still reported;
  - every `to` is allowed, for uniform client code: a single-file container gives a
    one-entry zip whose entry is byte-identical to the `file` response.
- The conversion slot, the 64 KiB streamed send and the send deadline apply to the zip
  as to any converted file.

### Generations: `output_packages: true`

- A new 3D request field `output_packages: Option<bool>` (default false) is accepted by
  every 3D model, like `ground` and `center`: litegen applies it on delivery.
- When true, delivery also writes each requested format's package. When that package
  has more than one file for this scene, it is delivered as an extra asset:
  - `kind: "package"` (new `Model3dAssetKind::Package`);
  - `format`: the container inside, e.g. `obj`;
  - stored as `{prefix}/{generation}/package_{format}.zip`, with `Content-Type:
    application/zip`;
  - `size_bytes` and `sha256` set;
  - its own `scene` manifest, measured from the package itself (the primary read
    through `MemorySidecars` over the package's files). Its `notes` are the
    package-mode losses, so an OBJ package's manifest shows its materials and texture
    maps and doesn't report them lost (MTL's own limits, such as double-sidedness, are
    still reported). This uses the existing `scene` field, so the
    API gains no new field. The `mesh` asset's manifest keeps describing the one-file
    container, as today (builder check: the two files differ, so one manifest can't
    describe both);
  - `kept: true`.

  A format whose package is one file (glb, fbx, stl, ply, 3mf, usdz, and obj, dae or
  usda of a scene with nothing to put beside them) gets no package asset.
- The `mesh` assets are unchanged, byte for byte. The promise "one mesh asset per
  requested format" stands.
- `keep_outputs: false` with `output_packages: true` is refused, 400
  `option_needs_keeping` (the code every opt-in output option uses; see the
  overview), before any vendor call. litegen makes every package, and
  keeping nothing writes nothing.
- The recorded `DeliveryPlan` gains `packages: bool`, so the client's poll and the
  poller deliver alike.
- No extra charge: packaging is conversion.

### Where the zip is written

`mesh::zip::write_package(package: &Package) -> Result<Vec<u8>, MeshError>`. It lives in
zip.rs, the one file allowed `miniz_oxide`:

- text entries (`.obj`, `.mtl`, `.dae`, `.usda`, `.gltf`) are `Deflated`;
- PNG, JPEG and `.bin` entries are `Stored { align: 1 }`;
- every entry name must pass `package::normalize_relative`, or the call is `Err`
  (defence in depth: writers already make safe names);
- the output is deterministic: two calls on one package are byte-identical.

### Clients

- **MCP:** the converter tool gains the `package` argument from the OpenAPI. For a zip,
  `file_result` labels the embedded resource `litegen://convert/<uuid>.<to>.zip` with
  `application/zip`. The 8 MiB limit applies unchanged. Regenerate `docs/mcp/tools.md`.
- **SDKs** (thin, per `.claude/rules/working-in-this-repo.md`):
  - TS `models3d.convert(input, { from, to, package: "zip" })` and Python
    `convert(…, package="zip")` pass the parameter;
  - the generate request types gain `output_packages`, and `Model3dAssetKind` gains
    `package`;
  - no unzipping in the SDKs.
- **Dashboard:**
  - `model3d-assets.ts` `KINDS` gains `package` (today it filters unknown kinds out);
  - the generation views show "Download {FORMAT} package (.zip)" per package asset;
  - `canPreviewMesh` is unchanged, so a zip is never handed to `<model-viewer>`.

## Non-goals

- Uploading a package to the converter (a zip of `.obj` + `.mtl` + textures as input).
  This is the natural follow-up, and it's listed in the overview.
- Delivering the vendor's own multi-file bundle as a package. litegen packages what it
  writes from the scene. The vendor's textures still arrive as `texture` assets, as
  today.
- Several-URL delivery (rejected above).

## What proves it is done

### mesh (zip.rs, tests.rs), RED first

- `write_package_round_trips_through_the_archive_reader`: for every fixture × {obj,
  dae, usda, gltf}, `zip::Archive::parse(write_package(p))` has the package's entries in
  order, with byte-equal contents and valid CRCs. Two calls return equal bytes.
- `a_package_zip_reads_back_as_its_container`: unzip → `MemorySidecars` →
  `read_scene(primary, format, &sidecars)`. Against `to.prepare(fixture, self_contained:
  false)`, it has equal material count, every texture binding resolved, image bytes
  equal, and the triangle count equal. For the `textured` fixture, the base-colour
  texture survives OBJ, DAE and USDA packages.
- `package_entry_names_never_escape`: a `Package` with `../x.png`, `/abs.png` or
  `C:x.png` → `Err`.

### Converter API, RED first

- `package_zip_returns_the_whole_package`, `to=obj` on the textured fixture's GLB:
  - 200, `content-type: application/zip`, disposition `model.obj.zip`;
  - entries `model.obj`, `model.mtl` and `textures/…`;
  - `X-Litegen-Conversion-Losses` equals the library's package-mode losses exactly. It
    has no `textures`, and its only `materials` entry is the fixture's double-sidedness,
    which MTL can't state in either mode. The `file` response's header also reports the
    textures and the materials baked into vertex colours.
- `package_file_is_todays_response`: `package=file` and no `package` give equal bytes
  and headers.
- `a_single_file_container_zips_to_one_entry`: `to=glb&package=zip` gives one entry,
  byte-equal to the `file` response.
- `package_values_are_checked`: `package=tar` → 400 `param_invalid`; given twice → 400
  `param_repeated`.
- The send-deadline test gains a zip case. The slot is freed when the deadline passes.

### Generation API (tests/model3d_api.rs), RED first

- `output_packages_delivers_a_zip_per_multi_file_container`: a mock model returning a
  textured GLB (add one if no mock row does, e.g. `mock/textured-3d` from
  `mesh::fixtures::textured`), with `output_formats: [glb, obj, dae, usda, gltf]` and
  `output_packages: true`:
  - package assets exist for obj, dae, usda and gltf, and none for glb;
  - each zip, fetched through its asset URL, reads back with its textures resolved;
  - each package's `scene.materials[*].maps` lists `base_color`, and its `scene.notes`
    report no textures lost and no materials baked into vertex colours (a container's
    own limit, such as MTL's double-sidedness, may still appear);
  - the mesh assets' `sha256`s and manifests equal those of the same request without
    the flag.
- `packages_follow_the_access_mode`: `signed` (the link fetches 200, sha256 matches) and
  `private` (`GET /v1/generations/{id}/assets/{index}` → 200 `application/zip`).
- `a_keep_nothing_request_cannot_ask_for_packages`: `X-Litegen-Keep-Outputs: false` +
  `output_packages: true` → 400 `option_needs_keeping` with `param: output_packages`,
  and the mock's job count is unchanged.
- `the_poller_delivers_packages`: only the poller completes the row, and the package
  assets are the same.
- `the_assets_list_names_packages`: `GET /v1/generations/{id}/assets` lists
  `kind: "package"`.

### OpenAPI, SDKs, MCP, dashboard

- openapi tests:
  - the converter's `package` parameter enum is `[file, zip]`, and its 200 response
    lists `application/zip`;
  - `Model3dGenerationRequest.output_packages` is documented;
  - `Model3dAssetKind` includes `package`.
- SDK suites after the frontend/SDK session regenerates:
  - TS: a convert test asserts `blob.type === "application/zip"` and `filename ===
    "model.obj.zip"`, and a types test covers `output_packages` and `"package"`;
  - Python: the same.
- MCP: `every_openapi_operation_is_a_tool_or_excluded` and `tools_reference_is_current`
  pass after regenerating `docs/mcp/tools.md`. A convert call with `package: "zip"`
  returns an embedded resource of `application/zip`.
- Dashboard: vitest `validAssets` keeps package assets, and the new
  `packageDownloads(assets)` returns them in request order. Playwright: a mock
  generation with `output_packages` shows the zip link, and fetching it returns 200
  `application/zip`.

### Render harness and independent readers

- **The harness already renders packages.** `examples/mesh_matrix.rs` writes every case
  in package mode (`WriteOptions { self_contained: false }`), unzipped into its case
  folder, with single-file outputs in `src_single/`. `page/render.js` already loads OBJ
  through `MTLLoader` and USDA with its sidecars. So what the zip adds to prove is that
  it carries exactly those files. `mesh_matrix` gains one step: each case's package
  goes through `zip::write_package`, the zip is unpacked by `python3 -I -m zipfile -e`
  into a scratch folder, and the folder is compared byte for byte with the case folder.
  Any difference fails the case. The existing package renders stay the visual proof,
  with 0 failures on the fixture set.
- Second opinions: SceneKit on OBJ + MTL, DAE and USDA packages; usdrecord on USDA
  packages; `usdchecker` (without `--arkit`, which is for USDZ) on the USDA package;
  `assimp info` on every primary with its sidecars on disk; `unzip -t` and
  `python3 -I -m zipfile -t` on every zip.
- Negative control: `write_package` drops the last sidecar. `a_package_zip_reads_back_as_its_container`
  and the harness's zip-versus-case-folder comparison fail.

### Docs

- `3d-format-conversion.md`: §9 loses "Multi-file delivery through the proxy", and §1's
  table gains a note that the converter and generations deliver packages on request.
- `mesh/mod.rs` "One file, or a package": the converter and delivery now ship packages
  on request.
- Also update:
  - README API section;
  - the convert endpoint's OpenAPI description, which says "The output is always one
    file";
  - the model3d-provider skill's delivery section;
  - the SDK READMEs and CHANGELOGs.
- Tell litegen-a3 the matrix page's "package only" texture cells for OBJ, DAE and USDA
  are now kept (packages).

### Gate

The overview's full gate, including `tests/model3d_api.rs`, the convert handler tests,
the openapi and MCP tests, the SDK suites, dashboard vitest and Playwright, and the
harness run with its zip check.

## Left to the builder

- Whether the mock textured model is a new row or an option on an existing one.
- The dashboard's exact link placement, next to the existing mesh download.
