# 3D converter packages and browser tool: implementation plan

**Status: done on 2026-11-05 by session litegen-d0, not committed.** Ledger: `.superpowers/sdd/2026-11-05-3d-converter-packages/progress.md`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship litegen's mesh library unchanged as `@litegen/3d-converter`
(npm, wasm), as `litegen-3d-converter` (PyPI, PyO3), and as a browser tool,
`apps/3d-converter`, that dogfoods the npm package.

**Architecture:**
- `packages/3d-converter/` is one Cargo workspace. In it:
  - `litegen-mesh` compiles `litegen-core/src/mesh/` as a crate through
    `[lib] path`, with no code changes;
  - `litegen-3d-converter` (core) is the typed high-level API;
  - two thin binding crates sit on top, wasm-bindgen + tsify and
    PyO3 + pyo3-stub-gen. Their types are generated from Rust.
- The app is Vite + React. It runs the package in a module worker and renders
  every format with three.js's own loader for it.

**Tech Stack:** Rust 1.98.1 (rustup), wasm-bindgen 0.2.130, tsify 0.5.8,
PyO3 0.30.1, pyo3-stub-gen 0.23.2, maturin 1.16.0, Vite 8, React 19,
three 0.183.2, Storybook 10.6, Vitest 4, Playwright 1.60.0, ffmpeg.

**Spec:** `docs/superpowers/specs/2026-11-05-3d-converter-packages-design.md`
(approved 2026-11-05). Read it in full first; this plan argues from it.

## Global Constraints

**Where you may write**

- Only in these paths:
  - `packages/3d-converter/**`;
  - `apps/3d-converter/**`;
  - the spec and this plan;
  - the ledger, `.superpowers/sdd/2026-11-05-3d-converter-packages/**`.
- Never edit `litegen-core/**`, `dashboard/**`, `sdks/**`, `scripts/**`, the
  root `.gitignore` or the root `package.json`.
- No worktrees, no commits, nothing staged, no `git checkout`, `git restore`
  or `git stash`.

**Running cargo**

- Start every Bash call with
  `export PATH=/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:/usr/local/bin:$PATH;`
  so cargo is rustup's, pinned by `packages/3d-converter/rust-toolchain.toml`.
- Always set `CARGO_INCREMENTAL=0`.
- Before any cargo run:
  1. wait out peers with
     `until ! { pgrep -qx cargo || pgrep -qx rustc || pgrep -qx clippy-driver; }; do sleep 20; done`
     (never `pgrep -f`);
  2. run `grep -rn NEGCTL litegen-core/src scripts/mesh-verify`. A hit means
     stop and ask its owner;
  3. don't build inside a peer's announced window ("starting" to "done";
     litegen-b2 announces them);
  4. check `df -h .`. Stop below 8 GiB.
- Record the mesh tree hash (`./snap`) of every build that feeds a test
  result.

**Pinned versions** (exact):
- `wasm-bindgen =0.2.130` and wasm-bindgen-cli 0.2.130;
- `tsify 0.5.8` with features `js`;
- `serde-wasm-bindgen 0.6.5`, `serde_bytes 0.11`, `serde_path_to_error 0.1`,
  `js-sys 0.3.107`;
- `pyo3 0.30.1` with feature `abi3-py310`, and `pyo3-stub-gen 0.23.2` behind
  the feature `stubgen`;
- `maturin 1.16.0`, through `uvx --from maturin==1.16.0`;
- three `0.183.2`, `@types/three` `0.183.1`, `@playwright/test` `1.60.0`;
- the dashboard's ranges for vite `^8.0.4`, react `^19.2.4`, typescript
  `~6.0.2`, storybook `^10.6.0` and vitest `^4.1.11`.

**`litegen-mesh` dependencies** are exactly litegen-core's:
- `serde_json = "1"`, `miniz_oxide = "0.8"`, `quick-xml = "0.32"`;
- `image = { version = "0.25", default-features = false, features = ["png", "gif", "webp", "jpeg"] }`;
- `edition = "2021"`.

**Floors:** Node ≥ 22, ESM only. Python ≥ 3.10, abi3 wheels.

**Names**
- npm `@litegen/3d-converter`. PyPI `litegen-3d-converter`, imported as
  `litegen_3d_converter`.
- Crates `litegen-mesh`, `litegen-3d-converter`, `litegen-3d-converter-js`
  and `litegen-3d-converter-py`. The Python crate's lib name is
  `litegen_3d_converter_py`; its module is named `litegen_3d_converter`.

**Option names**
- TypeScript: `from`, `to`, `fileName`, `package`, `units`, `up`,
  `fromUnits`, `fromUp`, `fbxColorSpace`, `fps`.
- Python: `from_format`, `to_format`, `file_name`, `package`, `units`, `up`,
  `from_units`, `from_up`, `fbx_color_space`, `fps`.
- Errors name the option in the binding's own spelling.

**Closed sets**, spelled exactly:
- formats: `glb gltf obj stl ply 3mf dae fbx usdz usda`;
- units: `m cm mm in ft`;
- up axes: `y z`;
- FBX colour spaces: `srgb linear`;
- packaging: `file zip files`;
- error codes: `format_unrecognized format_mismatch input_ambiguous input_too_large mesh_malformed mesh_unsupported mesh_empty param_unsupported param_invalid`;
- loss kinds: mesh's 15 `LossKind::as_str` spellings;
- file roles: `model material_library buffer texture layer unused`;
- stored frames: `y_up z_up declared caller`;
- material support: `none vertex_color color_only phong pbr`;
- bind poses: `t a unknown`.

**Exhaustiveness.** Never `_ =>` on a mesh enum, and never
`..Default::default()` on a mesh struct, in core or the bindings.

**The ledger.** RED and GREEN are recorded per test, in
`.superpowers/sdd/2026-11-05-3d-converter-packages/progress.md`: the command,
the failing name, the failure line, then the `test result:` line.

**No commit steps.** This laptop never commits. "Record" means a ledger line.

## Review Focus

Each of these has a test, added to the task that owns the code.

1. **Sidecar names whose case or folder differs from the reference** (OBJ/MTL
   from Windows, flat browser drops): they still resolve. Task 4,
   `sidecar_fallback_ignores_case_and_folder`.
2. **Non-ASCII file names** (`Textür.png`, `モデル.bin`) in multi-file and zip
   input: they still resolve. Task 4, `non_ascii_sidecar_names_resolve`.
3. **The same `Model` used from many Python threads at once, and a JS `Model`
   used after `free()`.** The threads give identical bytes; the freed model
   throws a clear error and doesn't corrupt memory. Task 8,
   `test_concurrent_writes_on_one_model_match`; Task 6,
   `write after free throws`.
4. **An empty file, an empty file set, or files with no model among them**:
   the error is `format_unrecognized`, not a panic or `mesh_malformed`.
   Task 4, `no_model_among_inputs_is_unrecognized`.
5. **A worker that dies mid-conversion** (a wasm trap or out of memory). The
   app reports it, then works on the next action. Task 11,
   `client recovers after a worker crash`.

---

### Task 1: Toolchain, workspace, the `litegen-mesh` crate

**Files:**
- Create in `packages/3d-converter/`:
  - `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`;
  - `mesh/Cargo.toml`;
  - `core/Cargo.toml`, `core/src/lib.rs`, `core/tests/mesh_crate.rs`.
- Create in `.superpowers/sdd/2026-11-05-3d-converter-packages/`:
  `progress.md`, `snap` and `pkg`, copied from
  `.superpowers/sdd/2026-11-04-mesh-uv-sets-usd-dae/` with `ws` repointed.

**Interfaces:**
- Produces:
  - the crate `litegen_mesh` (the `mesh` module's whole public API);
  - the workspace profiles `release` and `release-wasm`;
  - `packages/3d-converter/.tools/bin/wasm-bindgen`.

- [ ] **Step 1: Install the tools.**
  1. `brew install rustup binaryen`.
  2. `/opt/homebrew/opt/rustup/bin/rustup toolchain install 1.98.1 --profile minimal --target wasm32-unknown-unknown --component clippy,rustfmt`.
  3. Get wasm-bindgen 0.2.130: untar
     `https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.130/wasm-bindgen-0.2.130-aarch64-apple-darwin.tar.gz`
     into `packages/3d-converter/.tools/bin/`. If no such asset exists, use
     `cargo install wasm-bindgen-cli --version 0.2.130 --locked --root packages/3d-converter/.tools`,
     under the cargo protocol.
  4. `uv python install 3.10`.
- [ ] **Step 2: Verify the tools.**
  - `rustup run 1.98.1 rustc --version` prints `rustc 1.98.1`.
  - `rustup target list --installed --toolchain 1.98.1` includes
    `wasm32-unknown-unknown`.
  - `.tools/bin/wasm-bindgen --version` prints `wasm-bindgen 0.2.130`.
  - `wasm-opt --version` prints a version.
  - `/opt/homebrew/bin/cargo --version` still prints Homebrew's
    `cargo 1.98.1`, so peers are unchanged.
  - Record all of it.
- [ ] **Step 3: Write the workspace.**
  - `Cargo.toml`:
    - `[workspace] members = ["mesh", "core"]` (`js` and `python` join in
      Tasks 6 and 8), `resolver = "3"`;
    - `[workspace.package] version = "0.1.0"`, `license = "MIT"`,
      `edition = "2024"`, `rust-version = "1.98"`;
    - `[profile.release]` with `lto = "fat"`, `codegen-units = 1`,
      `opt-level = 3`, `panic = "unwind"`, `strip = "symbols"`;
    - `[profile.release-wasm]` with `inherits = "release"`,
      `panic = "abort"`, `strip = "none"`. wasm-bindgen strips; Task 7 sets
      `opt-level`.
  - `rust-toolchain.toml`: channel `1.98.1`, profile `minimal`, the
    `wasm32-unknown-unknown` target, the `clippy` and `rustfmt` components.
  - `.gitignore`: `target/`, `.tools/`, `js/dist/`, `python/dist/`,
    `.venv*/`, `__pycache__/`, `*.so`.
- [ ] **Step 4: Write the failing tests** in `core/tests/mesh_crate.rs`.
  - `mesh_deps_match_litegen_core`: parse `../../litegen-core/Cargo.toml` and
    `../mesh/Cargo.toml` with the `toml` dev-dependency. For each of
    `serde_json`, `miniz_oxide`, `quick-xml` and `image`, assert the two
    dependency values are equal. Also assert that `mesh`'s
    `[dependencies]` key set is exactly those four.
  - `mesh_lock_versions_match_litegen_core`: parse both `Cargo.lock` files.
    For every package name in `mesh`'s resolved dependency closure (read from
    this workspace's lock, starting at `litegen-mesh`), assert its version
    appears in litegen-core's lock with the same version.
  - `mesh_crate_converts_like_mesh`:
    `litegen_mesh::convert(&glb, MeshFormat::Glb, MeshFormat::Obj)` is `Ok`
    and non-empty. `glb` is
    `litegen_mesh::write_scene(&litegen_mesh::fixtures::textured(), MeshFormat::Glb, Default::default())`'s
    first file.

  Write `mesh/Cargo.toml` with `image` missing its `jpeg` feature, so the
  first test is RED by assertion, and the crate still builds.
- [ ] **Step 5: Run RED.**
  `cd packages/3d-converter && CARGO_INCREMENTAL=0 cargo test -p litegen-3d-converter --test mesh_crate`.
  Expected: `mesh_deps_match_litegen_core` FAILS, naming `image`. Record it.
- [ ] **Step 6: Make it pass.** Write `mesh/Cargo.toml` exactly as the Global
  Constraints say: `[lib] path = "../../../litegen-core/src/mesh/mod.rs"`,
  plus the four dependencies. If the lock test then names a version, align
  it with `cargo update -p <name> --precise <litegen-core's version>`.
  Rerun: `test result: ok. 3 passed`.
- [ ] **Step 7: Check that mesh builds for wasm.**
  `cargo build -p litegen-mesh --target wasm32-unknown-unknown --profile release-wasm`
  succeeds. Record the tree hash.

### Task 2: Core types: formats, options, errors

**Files:**
- Create: `core/src/{lib.rs, format.rs, options.rs, error.rs, loss.rs}`.
- Test: `core/tests/types.rs`.

**Interfaces:**
- Produces, with all names re-exported from the crate root:

  ```rust
  pub enum Format { Glb, Gltf, Obj, Stl, Ply, ThreeMf, Dae, Fbx, Usdz, Usda }
  impl Format { pub const ALL: [Format; 10]; pub fn as_str(self) -> &'static str;
      pub fn parse(s: &str) -> Option<Format>; pub fn media_type(self) -> &'static str;
      pub fn label(self) -> &'static str; pub fn info(self) -> FormatInfo; }
  pub fn formats() -> Vec<FormatInfo>;
  pub struct FormatInfo { pub format: Format, pub label: &'static str, pub extensions: Vec<&'static str>,
      pub media_type: &'static str, pub writes_sidecars: bool, pub stored_frame: StoredFrame,
      pub keeps: Keeps, pub keeps_in_package: Keeps,
      pub read_options: Vec<OptionName>, pub write_options: Vec<OptionName> }
  pub enum StoredFrame { YUp, ZUp, Declared, Caller }
  pub struct Keeps { pub hierarchy: bool, pub normals: bool, pub uv_sets: u32, pub vertex_colors: bool,
      pub materials: MaterialSupport, pub textures: bool, pub skins: bool, pub morph_targets: bool,
      pub animation: bool, pub multiple_meshes: bool, pub texture_filters: bool, pub texture_wrap: bool,
      pub texture_transforms: bool, pub layers: Vec<MaterialLayer> }
  pub enum MaterialSupport { None, VertexColor, ColorOnly, Phong, Pbr }
  pub enum MaterialLayer { Clearcoat, Transmission, Volume, Ior, Specular, Sheen, Iridescence, Anisotropy, Dispersion }
  pub enum Unit { M, Cm, Mm, In, Ft }          // ALL, as_str, parse, metres() -> f64
  pub enum UpAxis { Y, Z }                     // ALL, as_str, parse
  pub enum FbxColorSpace { Srgb, Linear }      // ALL, as_str, parse
  pub enum Packaging { File, Zip, Files }      // ALL, as_str, parse
  pub enum OptionName { From, To, FileName, Package, Units, Up, FromUnits, FromUp, FbxColorSpace, Fps } // ALL
  pub struct ReadOptions { pub from: Option<Format>, pub file_name: Option<String>, pub from_units: Option<Unit>,
      pub from_up: Option<UpAxis>, pub fbx_color_space: Option<FbxColorSpace> }        // Default
  pub struct WriteOptions { pub to: Format, pub package: Packaging, pub units: Option<Unit>, pub up: Option<UpAxis>,
      pub fbx_color_space: Option<FbxColorSpace>, pub fps: Option<u32> }              // WriteOptions::new(to)
  pub enum LossKind { /* mesh's 15, same order */ }  // as_str
  pub struct Loss { pub kind: LossKind, pub what: String, pub why: String }
  pub enum ErrorCode { FormatUnrecognized, FormatMismatch, InputAmbiguous, InputTooLarge, MeshMalformed,
      MeshUnsupported, MeshEmpty, ParamUnsupported, ParamInvalid }                     // as_str
  pub struct Error { pub code: ErrorCode, pub message: String, pub param: Option<OptionName> } // Display, std::error::Error
  impl From<litegen_mesh::MeshError> for Error;
  pub const MAX_INPUT_BYTES: usize = litegen_mesh::MAX_INPUT_BYTES;
  ```

  Every enum derives `Debug, Clone, Copy, PartialEq, Eq, Hash`.

- [ ] **Step 1: Write the failing tests** in `core/tests/types.rs`.
  - `format_names_round_trip`:
    - `Format::ALL.map(as_str)` equals
      `["glb","gltf","obj","stl","ply","3mf","dae","fbx","usdz","usda"]`;
    - `parse(f.as_str()) == Some(f)` for each;
    - `parse(" GLB ") == Some(Glb)`;
    - `parse("usdc") == None` (mesh's rule: a name means both directions).
  - `formats_lists_all_in_order`: `formats()`'s formats equal
    `Format::ALL`.
  - `media_types_match_litegens_table`:

    | Format | Media type |
    |---|---|
    | glb | `model/gltf-binary` |
    | gltf | `model/gltf+json` |
    | obj | `model/obj` |
    | stl | `model/stl` |
    | ply | `application/octet-stream` |
    | 3mf | `model/3mf` |
    | dae | `model/vnd.collada+xml` |
    | fbx | `application/octet-stream` |
    | usdz | `model/vnd.usdz+zip` |
    | usda | `model/vnd.usda` |
  - `keeps_mirror_mesh_capabilities`: for each format,
    - `info().keeps` equals the mapping of
      `MeshFormat::capabilities(mesh::WriteOptions { self_contained: true, fps: None, fbx_color_space: Srgb })`;
    - `keeps_in_package` equals the same with `self_contained: false`.

    Spot checks:
    - stl: `normals == false`, `materials == None`;
    - obj: `keeps.materials == VertexColor`,
      `keeps_in_package.materials == Phong`;
    - glb: `keeps.layers.len() == 9`.
  - `stored_frames`: glb and gltf `YUp`; 3mf `ZUp`; dae, fbx, usdz and usda
    `Declared`; stl, obj and ply `Caller`.
  - `option_lists`:
    - stl, obj and ply: `read_options == [FromUnits, FromUp]`,
      `write_options == [Units, Up]`;
    - fbx: `read == [FbxColorSpace]`, `write == [FbxColorSpace, Fps]`;
    - dae, usdz and usda: `write == [Fps]`;
    - glb, gltf and 3mf: both empty.
  - `extensions`: usda is `["usda","usdc","usd"]`; every other format is its
    own `as_str`.
  - `units_match_mesh`: for each `Unit`, `metres()` equals
    `normalize::metres_per_unit(as_str()).unwrap()`.
  - `mesh_errors_map_to_api_codes`:
    - `NotThisFormat` becomes `FormatMismatch`;
    - `Malformed` becomes `MeshMalformed`;
    - `Unsupported` becomes `MeshUnsupported`;
    - `Empty` becomes `MeshEmpty`;
    - `message` equals the mesh error's `Display`;
    - `param` is `None`.
  - `error_codes_spell_like_the_api`: `as_str` gives the Global Constraints
    spellings, in order.
  - `loss_kinds_spell_like_mesh`: for each mesh `LossKind`, the mapped
    `as_str` equals the mesh one.

  Start with stub modules whose functions return wrong values
  (`unimplemented!()` is fine here: this crate is ours alone), so the tests
  compile.
- [ ] **Step 2: Run RED.** `cargo test -p litegen-3d-converter --test types`.
  Every test fails. Record.
- [ ] **Step 3: Implement.**
  - Labels: `glTF Binary`, `glTF`, `Wavefront OBJ`, `STL`, `PLY`, `3MF`,
    `COLLADA`, `FBX`, `USDZ`, `USD`.
  - `From<MeshFormat>` and `Into<MeshFormat>` are exhaustive.
  - `Keeps` is built by destructuring `mesh::Capabilities` and
    `LayerSupport` with no `..`.
- [ ] **Step 4: Run GREEN.** The same command gives `test result: ok`. Then
  `cargo clippy -p litegen-3d-converter --all-targets -- -D warnings` exits 0.
  Record.

### Task 3: Format detection

**Files:**
- Create: `core/src/detect.rs`.
- Test: `core/tests/detect.rs`.

**Interfaces:**
- Produces: `pub fn detect_format(data: &[u8], file_name: Option<&str>) -> Option<Format>`.

- [ ] **Step 1: Write the failing tests.**
  - Input files come from `include_bytes!("../../../../litegen-core/src/mesh/testdata/…")`
    and from mesh writes of the fixtures.
  - `detects_every_written_format_without_a_name`: for the fixtures
    `textured` and `skinned`, written to every format self-contained,
    `detect_format(bytes, None) == Some(format)`.
  - `detects_independent_tools_files`:

    | File | Detected as |
    |---|---|
    | `fbx/textured_assimp_7500.fbx` | Fbx |
    | `fbx/cube_assimp_ascii.fbx` | Fbx |
    | `dae/scenekit/skinned_scenekit.dae` | Dae |
    | `3mf/assimp_textured.3mf` | ThreeMf |
    | `usd/textured_apple.usdz` | Usdz |
    | `usd/cube.usdc` | Usda |
    | `usd/comp.usda` | Usda |
    | `ply/cube_be_struct.ply` | Ply |
  - `magic_beats_the_name`: `detect_format(glb, Some("x.obj")) == Some(Glb)`.
  - `name_decides_when_bytes_cannot`:
    - `(b"", Some("m.usdc"))` gives `Some(Usda)`;
    - `(b"hello", Some("A.STL"))` gives `Some(Stl)`.
  - `unknown_is_none`:
    - `(b"hello world", None)` gives `None`;
    - `(br#"{"a":1}"#, None)` gives `None`;
    - a zip holding only `readme.txt` gives `None`.
  - `a_package_zip_is_not_a_model_format`: `zip::write_package` of an obj
    package gives `None`. Task 4 unpacks such zips.
- [ ] **Step 2: Run RED.** `cargo test -p litegen-3d-converter --test detect`.
  The stub returns `None`, so the positive tests fail. Record.
- [ ] **Step 3: Implement `detect_format`.**

  The order of checks:
  1. Strong magic:
     - `glTF` at 0;
     - `Kaydara FBX Binary  \0`;
     - `PXR-USDC`;
     - `ply\n` or `ply\r\n`;
     - `#usda`;
     - `; FBX` at the start;
     - `<COLLADA` in the first 4 KiB;
     - a JSON object (first non-space byte `{`) whose text contains
       `"asset"` anywhere. A self-contained `.gltf` can put megabytes of
       data URI before that key.
  2. A zip, checked with `zip::Archive::parse`:
     - an entry `3D/3dmodel.model`, or `[Content_Types].xml` with a `.model`
       entry, means 3mf;
     - a first entry ending in `.usda`, `.usdc` or `.usd` means usdz;
     - otherwise `None`.
  3. The file name's extension, case-insensitive: `usdc` and `usd` mean
     usda.
  4. Weak heuristics:
     - binary STL: `len >= 84` and `84 + 50 * u32_le(80..84) == len`;
     - ASCII STL: starts `solid` and contains `facet`;
     - OBJ: within the first 64 KiB, a line starting `v ` and one starting
       `f `.
- [ ] **Step 4: Run GREEN, then clippy.** Record.

### Task 4: Reading: `Model::read`, multi-file input, roles, facts

**Files:**
- Create: `core/src/{input.rs, info.rs, model.rs}`.
- Test: `core/tests/read.rs`.

**Interfaces:**
- Consumes: Tasks 2 and 3.
- Produces:

  ```rust
  pub enum Input<'a> { Bytes(&'a [u8]), Files(&'a [InputFile]) }
  pub struct InputFile { pub name: String, pub data: Vec<u8> }
  pub enum FileRole { Model, MaterialLibrary, Buffer, Texture, Layer, Unused }   // as_str
  pub struct InputFileInfo { pub name: String, pub role: FileRole, pub size: u64 }
  pub struct Model { /* scene after read_frame; OnceLock<(Scene, Vec<String>)> transcoded; … */ }
  impl Model { pub fn read(input: Input<'_>, options: &ReadOptions) -> Result<Model, Error>;
      pub fn format(&self) -> Format; pub fn info(&self) -> &ModelInfo; pub fn warnings(&self) -> &[String];
      pub fn files(&self) -> &[InputFileInfo]; }
  pub struct ModelInfo { pub triangles: u32, pub vertices: u32, pub nodes: u32, pub meshes: u32, pub textures: u32,
      pub uv_sets: u32, pub vertex_colors: bool, pub bounds: Option<Bounds>, pub materials: Vec<MaterialInfo>,
      pub skin: Option<SkinInfo>, pub animations: Vec<AnimationInfo> }
  pub struct Bounds { pub min: [f64; 3], pub max: [f64; 3] }
  pub struct MaterialInfo { pub name: Option<String>, pub maps: Vec<String> }
  pub struct SkinInfo { pub joints: u32, pub root: Option<String>, pub bind_pose: BindPose, pub humanoid: Option<String> }
  pub enum BindPose { T, A, Unknown }
  pub struct AnimationInfo { pub name: Option<String>, pub duration_s: f64, pub root_travel_m: [f64; 3], pub in_place: bool }
  ```

  `Model` must be `Send + Sync`: a `static_assertions`-style compile check in
  the test.

- [ ] **Step 1: Write the failing tests.**

  *Single files*
  - `reads_one_file_by_detection`: `read(Bytes(skinned glb), &default)`:
    - format is `Glb`;
    - `info.triangles` equals mesh's `triangle_count()`;
    - `files() == [{ name: "model.glb", role: Model, size }]`. Unnamed
      bytes take the name `model.<ext>`, or `file_name` when given.
  - `from_mismatch_is_format_mismatch`: glb bytes with `from: Some(Fbx)`
    give `FormatMismatch`.
  - `no_model_among_inputs_is_unrecognized` (Review Focus 4): each of these
    gives `FormatUnrecognized`:
    - `Bytes(b"")`;
    - `Bytes(b"hello")`;
    - `Files(&[])`;
    - `Files([readme.txt, tex.png])`.
  - `oversize_input_is_refused_before_parsing`: a file of
    `MAX_INPUT_BYTES + 1` zeros gives `InputTooLarge`, and so does a total
    over the cap across `Files`.

  *Several files*
  - `files_pick_the_model_and_give_roles`: the textured fixture, written as
    a gltf package, plus an extra `readme.txt`, in shuffled order:
    - format `Gltf`;
    - roles: gltf `Model`, bin `Buffer`, png `Texture`, txt `Unused`.
  - `obj_package_roles`: obj, mtl and png give the roles `Model`,
    `MaterialLibrary`, `Texture`, and `info.textures >= 1`.
  - `a_package_zip_unpacks`: `Bytes(zip::write_package(obj package))` reads
    as `Obj`, with `info.textures >= 1`.
  - `ambiguous_models`:
    - `[a.obj, b.obj]` gives `InputAmbiguous`, with or without
      `from = Obj`;
    - `[a.obj, b.glb]` with `from = Glb` reads `b.glb`.
  - `sidecar_fallback_ignores_case_and_folder` (Review Focus 1): an obj
    package whose texture file is renamed from `textures/<name>.png` to
    upper-case `<NAME>.PNG` at the top level still resolves:
    `info.textures >= 1`, role `Texture`.
  - `non_ascii_sidecar_names_resolve` (Review Focus 2): a gltf package whose
    image `uri` is edited to `Textür.png` (percent-encoded in the JSON), with
    the file named `Textür.png`, resolves. So does the same set zipped.

  *Read options*
  - `read_option_rules`:
    - `from_units` on glb gives `ParamUnsupported`, param `FromUnits`;
    - `from_up` on obj is accepted;
    - `fbx_color_space` on glb gives `ParamUnsupported`, param
      `FbxColorSpace`;
    - `fbx_color_space` on fbx is accepted.
  - `from_units_and_up_set_the_frame`: an STL of the cube fixture.
    - `from_units = Mm`: the bounds are the default bounds × 0.001, within
      1e-9 relative.
    - `from_up = Z`: the y extent equals the default z extent.

  *Facts and warnings*
  - `info_matches_inspect`:
    - for skinned: `skin.joints`, `animations.len()` and each
      `duration_s` match `inspect::inspect`;
    - for textured: some `materials[i].maps` contains `"base_color"`.
  - `reader_warnings_carry_through`: an OBJ naming an absent `.mtl` gives
    non-empty `warnings()`.
- [ ] **Step 2: Run RED.** `cargo test -p litegen-3d-converter --test read`.
  Record.
- [ ] **Step 3: Implement.**
  - Primary choice, sidecars, roles and the zip unpack are as spec §7.3
    says.
  - The resolver implements `litegen_mesh::Sidecars`:
    1. an exact normalised path;
    2. otherwise a unique case-insensitive basename;
    3. it records each path asked for, in a `RefCell<BTreeSet>`. The
       resolver lives only during `read`.
  - Roles by extension:

    | Extension | Role |
    |---|---|
    | `mtl` | `MaterialLibrary` |
    | `bin` | `Buffer` |
    | `png`, `jpg`, `jpeg`, `webp`, `gif`, `ktx2` | `Texture` |
    | `usd`, `usda`, `usdc` | `Layer` |
    | anything else asked for | `Buffer` |
    | never asked for | `Unused` |
  - Read pipeline: `read_scene_with`, then `normalize::read_frame` when
    `from_units` or `from_up` is set.
  - Option checks run against `Format::info().read_options` after detection.
- [ ] **Step 4: Run GREEN, then clippy.** Record.

### Task 5: Writing: `Model::write`, `convert`, the handler tripwire

**Files:**
- Create: `core/src/convert.rs`.
- Modify: `core/src/model.rs`.
- Test: `core/tests/write.rs` and `core/tests/handler_tripwire.rs`.

**Interfaces:**
- Produces:

  ```rust
  impl Model { pub fn write(&self, options: &WriteOptions) -> Result<Conversion, Error>; }
  pub struct Conversion { pub format: Format, pub files: Vec<OutputFile>, pub losses: Vec<Loss>, pub warnings: Vec<String> }
  pub struct OutputFile { pub name: String, pub media_type: &'static str, pub role: FileRole, pub data: Vec<u8> }
  pub fn convert(input: Input<'_>, read: &ReadOptions, write: &WriteOptions) -> Result<Conversion, Error>;
  ```

  `convert` validates `fbx_color_space` against the source's read options
  together with the target's write options. It passes the value to the read
  only when the source is fbx, and to the write only when the target is fbx.
  `Conversion.warnings` is the reader's warnings.

- [ ] **Step 1: Write the failing tests** in `write.rs`.

  *Matching the hosted converter*
  - `default_convert_equals_the_hosted_pipeline`. The test holds a
    function, `hosted`, that copies `convert_bytes` in
    `litegen-core/src/api/handlers/convert.rs:360-392` call for call. Over
    the fixtures `textured`, `skinned`, `morph` and `cube`, each written as
    glb, and every target:
    - core's `convert(Bytes, from Glb, WriteOptions::new(to))` files equal
      `hosted`'s, byte for byte;
    - so do the `Zip` packaging and `units = Mm, up = Z` variants for stl,
      obj and ply.
  - `model_writes_equal_one_shot_convert`: one `Model::read`, then every
    target × every packaging, each equal to `convert`.

  *Packaging and file roles*
  - `package_modes`:
    - `File` gives one file `model.<ext>`, role `Model`;
    - `Zip` gives one file `model.<ext>.zip`, `application/zip`, role
      `Model`, starting `PK\x03\x04`, whose entry names equal `Files`
      mode's names in order;
    - `Files` puts the main file first. For obj: `model.mtl` has role
      `MaterialLibrary` and each png has role `Texture`, with media types
      from the table: `mtl` `model/mtl`, `bin` `application/octet-stream`,
      `png` `image/png`, `jpg` `image/jpeg`.

  *Write options*
  - `write_option_rules`:
    - `units` on glb gives `ParamUnsupported(Units)`;
    - `up` on fbx gives `ParamUnsupported(Up)`;
    - `fps` on glb gives `ParamUnsupported(Fps)`;
    - `fps = 0` on fbx gives `ParamInvalid(Fps)`;
    - `fbx_color_space` on an obj target gives
      `ParamUnsupported(FbxColorSpace)`;
    - `convert` from fbx to glb with `fbx_color_space` succeeds.
  - `units_and_up_reach_the_file`: glb to stl with `units = Mm` reads back
    with bounds × 1000. With `up = Z`, the z extent equals the source's y
    extent.
  - `fbx_color_space_reaches_the_writer`: textured to fbx gives different
    bytes for `Srgb` and `Linear`, and `None` equals `Srgb`.
  - `fps_reaches_the_writer`: skinned to fbx and to dae at `fps = 24`
    differ from the default.

  *Losses*
  - `undecodable_texture_is_an_image_formats_loss`: a glb whose embedded
    image is a corrupt WebP gives a loss whose kind is `ImageFormats` and
    whose `what` starts `texture `.
  - `losses_mirror_mesh`: skinned to stl gives losses equal to mesh's
    `write_scene` losses, mapped, kinds including `Animation` and `Skins`.

  *The tripwire*, in `handler_tripwire.rs`:
  - `hosted_converter_logic_is_unchanged`:
    1. Read `../../../litegen-core/src/api/handlers/convert.rs`.
    2. Extract the `const PARAMS` line, and the `fn convert_bytes(` and
       `fn parse_query(` items through their column-0 `}`.
    3. Hash them with an FNV-1a 64 written in the test.
    4. Assert equal to `PINNED`.

    The failure message: "litegen's converter handler changed (PARAMS /
    convert_bytes / parse_query): re-check core/src/model.rs write and
    core/src/options.rs against it, then update PINNED".
- [ ] **Step 2: Run RED.** Record. For the tripwire, RED is a first run with
  `PINNED = 0`; GREEN pins the printed hash.
- [ ] **Step 3: Implement.** `write`, per spec §7.3:
  1. the cached transcode (`OnceLock`), whose dropped list becomes
     `ImageFormats` losses with the handler's exact strings;
  2. a clone plus `express_frame`, only when `units` or `up` is set;
  3. `mesh::write_scene` with a full `mesh::WriteOptions` literal;
  4. for `Zip`, `zip::write_package`.

  Then map the files and their roles.
- [ ] **Step 4: Run GREEN.** `cargo test -p litegen-3d-converter`: all core
  tests. Then clippy. Record the tree hash.

### Task 6: The JS binding and the npm package

**Files:**
- Create in `packages/3d-converter/js/`:
  - the crate: `Cargo.toml` and `src/{lib.rs, api.rs, convert_error.js}`;
  - the package: `package.json` and `README.md` (a stub; Task 15 fills it);
  - `entry/node.js`, `entry/web.js`, `scripts/build.mjs`;
  - tests: `test/api.test.mjs`, `test/types.test.ts`, `test/tsconfig.json`,
    `test/pack.test.mjs`.
- Modify: workspace `Cargo.toml` members, adding `"js"`.

**Interfaces:**
- Consumes: the core API.
- Produces the package API exactly as spec §7.4 writes it, plus:

  ```ts
  export type ModelInput = Uint8Array | Record<string, Uint8Array>;
  export interface InputFileInfo { name: string; role: FileRole; size: number }
  export class Model { readonly format: Format; readonly info: ModelInfo; readonly warnings: string[];
    readonly files: InputFileInfo[]; write(options: WriteOptions): Conversion; free(): void }
  export function version(): string;
  export function maxInputBytes(): number;
  export declare class ConvertError extends Error { readonly name: "ConvertError"; readonly code: ErrorCode; readonly param?: OptionName }
  ```

  - Outputs' `data` is `Uint8Array<ArrayBuffer>`.
  - `Option` fields are omitted when `None`
    (`skip_serializing_if` + `#[tsify(optional)]`).
  - Options are `deny_unknown_fields` and decoded through
    `serde_path_to_error`. A bad value is `param_invalid`; an unknown key is
    `param_unsupported`; either way `param` names the field.

- [ ] **Step 1: Write the failing tests.**
  - `test/api.test.mjs` uses `node:test` and imports
    `@litegen/3d-converter` through the package's self-reference. That also
    exercises its exports map. The inputs are testdata files read with
    `fs`. Tests:
    - `converts glb to every format`: from
      `fbx/skinned_small_source.glb`, each output's `files[0].data` starts
      with the right content:

      | Target | Starts with, or contains |
      |---|---|
      | glb | `glTF` |
      | fbx | `Kaydara FBX Binary` |
      | 3mf, usdz | `PK\x03\x04` |
      | ply | `ply` |
      | usda | `#usda` |
      | dae | contains `<COLLADA` |
      | gltf | parses as JSON with `asset.version === "2.0"` |
      | stl | `length === 84 + 50 * count` |
      | obj | contains `\nv ` |

      Also `format === to`, and `files[0].mediaType` per the table.
    - `errors are ConvertError`, each `instanceof ConvertError` and
      `instanceof Error`, `name === "ConvertError"`:
      - `from: "fbx"` on glb bytes gives `code === "format_mismatch"`;
      - `{to: "glb", units: "mm"}` gives
        `{code: "param_unsupported", param: "units"}`;
      - `{to: "xyz"}` gives `{code: "param_invalid", param: "to"}`;
      - `{to: "glb", scale: 2}` gives
        `{code: "param_unsupported", param: "scale"}`.
    - `model writes equal convert`.
    - `write after free throws` (Review Focus 3): `m.free()`, then
      `m.write(...)` throws an `Error`, and the next `readModel` still
      works.
    - `disposes with using`: `typeof m[Symbol.dispose] === "function"`, and
      after calling it, `write` throws. If wasm-bindgen 0.2.130 doesn't emit
      it, drop this test, record that in the ledger, and have the README
      document `free()` only.
    - `files input`: a `package: "files"` output, fed back as a `Record`,
      reads with the same `info.triangles`.
    - `zip input`: a `package: "zip"` output, fed back as a `Uint8Array`.
    - `detectFormat`: every testdata file from Task 3's table.
    - `formats and limits`: `formats().length === 10`, and
      `maxInputBytes() === 134217728`.
  - `test/types.test.ts` is compiled with `tsc -p test/tsconfig.json`
    (`strict`, `noEmit`, `exactOptionalPropertyTypes`). It holds valid uses,
    plus `// @ts-expect-error` lines for each of these:
    - `to: "xyz"`;
    - a missing `to`;
    - `units: "km"`;
    - an unknown key `scale`;
    - `fbxColorSpace: "rgb"`;
    - assigning `c.files[0].data` to `string`;
    - `loss.kind === "colour"` (no overlap).
  - `test/pack.test.mjs`: `npm pack --dry-run --json` lists exactly
    `package.json`, `README.md`, `dist/index.d.ts`, `dist/node.js`,
    `dist/web.js`, `dist/litegen_3d_converter.js`,
    `dist/litegen_3d_converter.d.ts`, `dist/litegen_3d_converter_bg.wasm`,
    and the one `dist/snippets/**/convert_error.js`.
- [ ] **Step 2: Run RED.** `npm test`, which runs
  `node --test test/*.test.mjs && tsc -p test/tsconfig.json`, fails because
  `dist/` doesn't exist. This package is isolated, so RED by a missing build
  is acceptable here. Record.
- [ ] **Step 3: Implement the crate.**
  - Thin binding types with serde derives and tsify (`rename_all`
    camelCase for fields; enums take the snake spellings), plus `From`
    conversions to and from core.
  - `ModelInput` is decoded from a `JsValue`: a `Uint8Array` becomes
    `Bytes`, an object of `Uint8Array`s becomes `Files`. The parameter is
    typed with `#[wasm_bindgen(unchecked_param_type = "ModelInput")]`.
  - `ConvertError` is the JS class in `src/convert_error.js`, imported with
    `#[wasm_bindgen(module = "/src/convert_error.js")]` and constructed for
    every `Err`.
  - A `typescript_custom_section` declares `ModelInput` and the
    `ConvertError` class.
- [ ] **Step 4: Implement the build and the entries.** `scripts/build.mjs`
  runs:
  1. the rustup cargo,
     `build -p litegen-3d-converter-js --target wasm32-unknown-unknown --profile release-wasm`;
  2. `.tools/bin/wasm-bindgen <wasm> --target web --out-dir dist --out-name litegen_3d_converter`;
  3. `wasm-opt` with the flags Task 7 settles; until then `-O3`, plus the
     `--enable-*` flags for rustc's default wasm features;
  4. writes `dist/node.js`, `dist/web.js` and `dist/index.d.ts` from
     `entry/`.

  It finds the snippet path under `dist/snippets/` and writes
  `export { ConvertError } from "./snippets/<dir>/src/convert_error.js"`
  into both entries. It then prints raw, gzip and brotli sizes.

  The entries:
  - `node.js`: `initSync({ module: readFileSync(new URL("./litegen_3d_converter_bg.wasm", import.meta.url)) })`;
  - `web.js`: `await init({ module_or_path: new URL("./litegen_3d_converter_bg.wasm", import.meta.url) })`.

  Both then `export *` from `./litegen_3d_converter.js`.

  `package.json`:
  - `"type": "module"`, `"engines": {"node": ">=22"}`,
    `"sideEffects": ["./dist/node.js", "./dist/web.js"]`;
  - `"files": ["dist"]`, `"license": "MIT"`;
  - `"exports"`: `"."` maps to
    `{"types": "./dist/index.d.ts", "node": "./dist/node.js", "default": "./dist/web.js"}`;
    `"./litegen_3d_converter_bg.wasm"` maps to the wasm file;
    `"./package.json"` maps to itself;
  - scripts `build`, `test`, `bench`;
  - devDependencies `typescript ~6.0.2`.
- [ ] **Step 5: Run GREEN.** `npm run build && npm test` passes. Then
  `cargo clippy -p litegen-3d-converter-js --target wasm32-unknown-unknown -- -D warnings`
  exits 0. Record the sizes and the tree hash.

### Task 7: wasm build settings, chosen by measurement

**Files:**
- Create: `js/scripts/bench.mjs`.
- Modify: `js/scripts/build.mjs` (its defaults) and `Cargo.toml`
  (`release-wasm`'s `opt-level`).

- [ ] **Step 1: Write `bench.mjs`.**
  - It loads a given `dist/`.
  - It converts `fbx/skinned_small_source.glb`,
    `fbx/textured_assimp_7500.fbx` and `usd/textured_apple.usdz` to all 10
    targets.
  - It takes 7 rounds, discards the first, and prints the median total in
    ms.
- [ ] **Step 2: Build and time the variants.** opt-level `3` or `s`, times
  `+simd128` on or off, gives 4 cargo builds. Each gets `wasm-opt -O3` and
  `-Oz`: 8 variants.
  - For each, record the raw, gzip and brotli sizes and the bench median.
  - Each must also pass `npm test`.
- [ ] **Step 3: Choose by the spec's rule.** Take the fastest variant, unless
  another is within 5% of its speed and at least 15% smaller.
  - Set the defaults in `build.mjs` and `Cargo.toml`.
  - Add a size budget to `test/pack.test.mjs`: the wasm's brotli size is at
    most the chosen variant's × 1.10.
  - Put the table and the choice in the ledger, and later in the README.
  - If `+simd128` wins, note in the README that it needs Safari ≥ 16.4,
    Chrome ≥ 91, Firefox ≥ 89 or Node ≥ 16.4.

### Task 8: The Python binding and package

**Files:**
- Create in `packages/3d-converter/python/`:
  - the crate: `Cargo.toml` and `src/{lib.rs, api.rs}`, plus
    `src/bin/stub_gen.rs`, which needs the `stubgen` feature;
  - `pyproject.toml` and `README.md` (a stub);
  - `litegen_3d_converter.pyi` (generated);
  - tests: `tests/{test_api.py, test_threads.py, test_typing.py, typed_usage.py}`.
- Modify: workspace members, adding `"python"`.

**Interfaces:**
- Produces the Python API exactly as spec §7.5 writes it, plus:
  - `Model.files: list[InputFileInfo]`;
  - `version() -> str`;
  - `MAX_INPUT_BYTES: int`;
  - `ConvertError(ValueError)`, with `code: ErrorCode` and
    `param: str | None`.

  Every closed set is a `Literal` alias in the stub: `Format`, `Unit`,
  `UpAxis`, `FbxColorSpace`, `Packaging`, `LossKind`, `ErrorCode`,
  `FileRole`, `StoredFrame`, `MaterialSupport`, `MaterialLayer`,
  `BindPose`, `OptionName`.

- [ ] **Step 1: Write the failing tests.**
  - `test_api.py`, the same behaviours as Task 6's `api.test.mjs`:
    - `test_converts_to_every_format`;
    - `test_errors`:
      - `ConvertError` is a subclass of `ValueError`;
      - `to_format="xyz"` gives
        `code == "param_invalid", param == "to_format"`;
      - `units="mm"` with `to_format="glb"` gives
        `code == "param_unsupported", param == "units"`;
      - an unknown keyword raises `TypeError`, PyO3's own behaviour;
    - `test_model_writes_equal_convert`;
    - `test_files_and_zip_input`;
    - `test_bytes_bytearray_memoryview_inputs_agree`;
    - `test_detect_format`;
    - `test_formats_and_limits`;
    - `test_info_and_roles`.
  - `test_threads.py`:
    - `test_gil_is_released_during_a_conversion`: a thread spins a counter
      while the main thread converts `usd/textured_apple.usdz` to fbx 20
      times. The counter must advance by at least 1000 during those calls.
      Under a held GIL it advances only at switch intervals.
    - `test_concurrent_writes_on_one_model_match` (Review Focus 3): 8
      threads write one `Model` to fbx 10 times each. Every result's bytes
      are identical.
  - `test_typing.py`:
    - `test_stub_is_current`: run `stub_gen` into a temporary directory and
      compare it to the committed `.pyi`;
    - `test_stubtest_passes`: `python -m mypy.stubtest litegen_3d_converter`
      exits 0;
    - `test_typed_usage_is_strict`: `mypy --strict --warn-unused-ignores
      typed_usage.py` and `pyright --strict typed_usage.py` exit 0. The file
      marks its deliberate mistakes with
      `# type: ignore[arg-type]  # pyright: ignore[reportArgumentType]`,
      so an unneeded ignore fails.
- [ ] **Step 2: Run RED.** Make a venv and install:
  `uv venv .venv314 --python 3.14 && uv pip install -p .venv314 pytest mypy pyright`.
  `pytest` fails on the import. Record.
- [ ] **Step 3: Implement.**
  - frozen `#[pyclass]` types with read-only getters, and
    `#[pyo3(signature = (data, *, to_format, from_format=None, …))]`;
  - `data` is extracted as `bytes` (`PyBackedBytes`, no copy), as other
    buffers through `PyBuffer<u8>`, or as a `Mapping[str, bytes]`;
  - the work runs inside `py.detach`, under `#[pymodule(gil_used = false)]`;
  - errors go through `create_exception!` with `code` and `param` set as
    attributes;
  - stub types are overridden to the `Literal` aliases with
    `#[gen_stub(override_type(…))]` and `type_alias!`/`gen_type_alias_from_python!`.
  - `pyproject.toml`:
    - `[build-system] requires = ["maturin>=1.16,<2"]`;
    - `[project] requires-python = ">=3.10"`, with the classifiers
      `Typing :: Typed` and `Programming Language :: Rust`;
    - `[tool.maturin] module-name = "litegen_3d_converter"`,
      `features = ["pyo3/extension-module"]`.
- [ ] **Step 4: Build, then run GREEN on two Pythons.**
  1. Generate the stub:
     `cargo run -p litegen-3d-converter-py --bin stub_gen --features stubgen`.
  2. Build the wheel:
     `cd python && uvx --from maturin==1.16.0 maturin build --release --out dist`.
  3. For each of 3.10 and 3.14:
     `uv venv .venv<v> --python <v>`, install the wheel plus pytest, mypy and
     pyright, then run `pytest -q`. Both pass.
  4. The wheel's name matches `litegen_3d_converter-0.1.0-cp310-abi3-macosx_*_arm64.whl`.

  Then `cargo clippy -p litegen-3d-converter-py --all-targets -- -D warnings`.
  Record.
- [ ] **Step 5: Public API review.** Dispatch one fresh reviewer over the
  spec, `core/src`, `js/src` with the generated `dist/*.d.ts`, `python/src`
  with the `.pyi`, and both test suites. Fix every Critical and Important
  finding through TDD. Record deferred minors with reasons.

### Task 9: App scaffold and `MeshViewer`

**Files:**
- Create in `apps/3d-converter/`:
  - setup: `package.json`, `.gitignore`, `index.html`, `vite.config.ts`,
    `tsconfig.json`, `tsconfig.app.json`, `tsconfig.node.json`;
  - `src/main.tsx` and `src/App.tsx` (a placeholder for now);
  - the viewer: `src/viewer/{MeshViewer.tsx, loaders.ts, framing.ts, webgl.ts, viewer.css}`;
  - `.storybook/{main.ts, preview.ts}`, and `.storybook/fixtures/`, copied
    from `dashboard/.storybook/fixtures` (`cube.glb`, `icosphere.glb`,
    `corrupt.glb`) and from `litegen-core/src/mesh/testdata`;
  - `src/viewer/MeshViewer.stories.tsx`;
  - `playwright.config.ts` and `e2e/stories.spec.ts`.

**Interfaces:**
- Consumes: `@litegen/3d-converter` types (`Format`, `StoredFrame`,
  `UpAxis`, `FileRole`).
- Produces:

  ```ts
  export interface ViewerFile { name: string; data: Uint8Array; role?: FileRole }
  export type ViewerBackground = 'dark' | 'light' | 'checker';
  export interface ViewerLoadInfo { dimensions: { x: number; y: number; z: number } | null; clips: number }
  export interface MeshViewerHandle { resetCamera(): void }
  export interface MeshViewerProps { files: ViewerFile[]; format: Format; storedFrame: StoredFrame; up?: UpAxis;
    autoRotate?: boolean; animate?: boolean; exposure?: number; background?: ViewerBackground; testId?: string;
    style?: React.CSSProperties; onLoad?(info: ViewerLoadInfo): void; onError?(message: string): void;
    ref?: React.Ref<MeshViewerHandle> }
  ```

  The root element carries these attributes:
  - `data-testid={testId}`;
  - `data-state`: `loading`, `loaded` or `error`;
  - `data-format`;
  - after load, `data-coverage`: the fraction, from 0 to 1, of a 64×64
    rest-pose silhouette that the model covers;
  - `data-silhouette`: the base64 of that 64×64 one-bit mask, row-major and
    MSB first. Task 13 reads it.

- [ ] **Step 1: Scaffold.**
  - `package.json`:
    - dependencies `"@litegen/3d-converter": "file:../../packages/3d-converter/js"`,
      `react`, `react-dom`, `three 0.183.2`, `lucide-react ^1.8.0`;
    - devDependencies at the Global Constraints versions;
    - scripts `dev`, `build` (`tsc -b && vite build`), `preview`, `test`
      (vitest), `storybook`, `build-storybook`, `test:e2e`,
      `test:stories`.
  - `vite.config.ts`:
    - `base: './'`, `worker: { format: 'es' }`;
    - `optimizeDeps: { exclude: ['@litegen/3d-converter'] }`;
    - `build.target: 'es2022'`;
    - `server.port 5181`, `preview.port 5182`;
    - `server.fs.allow`: `searchForWorkspaceRoot(process.cwd())` plus
      `../../packages/3d-converter/js`. The `file:` dependency resolves
      outside the app.
  - `npm install`.
- [ ] **Step 2: Write the failing story test.** `e2e/stories.spec.ts` is the
  Playwright project `stories`. Its `webServer` serves `storybook-static` on
  6107 with `python3 -m http.server`. It reads `index.json` and, for every
  story, opens `iframe.html?id=<id>` and asserts:
  - no `pageerror` and no console `error`;
  - for stories tagged `viewer`, `[data-state="loaded"]` within 20 s and
    `data-coverage >= 0.01`;
  - for stories tagged `viewer-error`, `[data-state="error"]`.
- [ ] **Step 3: Write the stories.** One `MeshViewer` story per format,
  tagged `viewer`, from the fixtures:

  | Format | Fixture |
  |---|---|
  | glb | `cube.glb` |
  | gltf | written in Task 12, so add this story then |
  | fbx | `textured_assimp_7500.fbx` |
  | dae | `skinned_scenekit.dae` |
  | 3mf | `assimp_textured.3mf` |
  | usdz | `textured_apple.usdz` |
  | usda | `cube.usdc` |
  | ply | `cube_be_struct.ply` |

  OBJ and STL stories come with Task 12's samples. Add `Corrupt`, a
  `corrupt.glb` tagged `viewer-error`.
- [ ] **Step 4: Run RED.**
  `npm run build-storybook && npx playwright test --project stories` fails,
  because MeshViewer is a stub. Record.
- [ ] **Step 5: Implement `MeshViewer`.**
  - `loaders.ts` ports `scripts/mesh-verify/page/render.js:259-372`
    (`loaderFor`, `load`, `texturesReady`), with these changes:
    - inputs are blob URLs of `files`, mapped through
      `LoadingManager.setURLModifier`, exact path first, then basename;
    - the OBJ `.mtl` is the file with role `material_library`, not one found
      by extension;
    - the 3MF −90° X turn becomes the generic rule: turn when
      `storedFrame === 'z_up'`, or when `storedFrame === 'caller' && up === 'z'`;
    - the format switch is exhaustive with a `never` check.
  - `framing.ts` ports `frameCamera` (`render.js:558-573`), using the
    dashboard's framing values (`ModelViewer.tsx:95-97`).
  - `webgl.ts` ports `hasWebGL2` (`ModelViewer.tsx:123-135`).
  - The renderer:
    - `WebGLRenderer`, `NeutralToneMapping`;
    - a `RoomEnvironment` PMREM;
    - `OrbitControls` with auto-rotate;
    - an `AnimationMixer` playing clip 0 when `animate`;
    - the dashboard's backgrounds (`ModelViewer.tsx:85-90`);
    - the fallback panel ported from `MeshFallback` (`ModelViewer.tsx:167-214`).
  - The silhouette renders the rest pose (actions stopped), with
    `MeshBasicMaterial` white on black.
  - Each ported file has a header comment naming its source.
- [ ] **Step 6: Run GREEN.** The same command: every viewer story is
  `loaded` with coverage ≥ 0.01, and `Corrupt` is `error`. Then
  `npm run build`, which runs `tsc -b`, passes. Record.

### Task 10: The inspector and the presentational components

**Files:**
- Create: `src/viewer/{Inspector.tsx, Inspector.css}`.
- Create: `src/components/{ModelFacts.tsx, LossList.tsx, FileList.tsx}`.
- Create: `src/format-helpers.ts` and `src/format-helpers.test.ts`.
- Stories for each.

**Interfaces:**
- `Inspector`: `{ title: string; viewer: MeshViewerProps; actions?: ReactNode; testId: string }`.
  - It owns autoRotate, exposure (0.25–2), background and fullscreen, ported
    from `ModelPreview.tsx:719-918`.
  - It shows the size and dimensions stats.
- `ModelFacts`: `{ info: ModelInfo; testId }`. The scene facts and the clips
  table, ported from `ModelPreview.tsx:596-652`.
- `LossList`: `{ losses: Loss[]; warnings: string[]; testId }`.
- `FileList`: `{ files: Array<{ name: string; role: FileRole; size: number }>; testId }`.
- `format-helpers.ts`: `formatBytes(n: number | null | undefined): string`
  and `formatDimensions(d): string`. Port the dashboard's implementations
  from `model3d-assets.ts` and keep their outputs.

- [ ] **Step 1: Write the failing vitest.** `format-helpers.test.ts` pins the
  dashboard functions' outputs for 0, 1023, 1536, 5 MB and null. Copy the
  expected strings from running the dashboard's
  `src/components/model3d-assets.test.ts` cases.
- [ ] **Step 2: Run RED** (`npx vitest run`), **implement, run GREEN.**
- [ ] **Step 3: Add the stories.**
  - `Inspector`, over the fbx fixture, tagged `viewer`;
  - `ModelFacts`, with skinned and textured facts;
  - `LossList`, with none, some, and warnings;
  - `FileList`, with an obj package.

  Port the inspector's CSS from `dashboard/src/components/ModelPreview.css`,
  keeping its `mp-*` class names and palette. Run
  `npx playwright test --project stories` GREEN. Record.

### Task 11: The worker, the client, file intake

**Files:**
- Create: `src/converter/{protocol.ts, worker.ts, client.ts, client.test.ts}`.
- Create: `src/intake.ts` and `src/intake.test.ts`.

**Interfaces:**

```ts
export interface ReadResult { format: Format; info: ModelInfo; warnings: string[]; files: InputFileInfo[] }
export interface WriteResult { conversion: Conversion; ms: number }
export interface ClientError { code: ErrorCode | 'worker_crashed'; message: string; param?: string }
export class ConverterClient {
  constructor(createWorker?: () => Worker);   // default: new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' })
  formats(): Promise<FormatInfo[]>;
  read(files: Record<string, Uint8Array>, options?: ReadOptions): Promise<ReadResult>;
  write(options: WriteOptions): Promise<WriteResult>;
  dispose(): void;
}
export function filesFromList(list: ArrayLike<File>): Promise<Record<string, Uint8Array>>;          // webkitRelativePath when set
export function filesFromEntries(entries: FileSystemEntryLike[]): Promise<Record<string, Uint8Array>>; // folder walk, relative paths
```

- The worker keeps one `Model`. `read` frees the previous one.
- `write` transfers every `data.buffer`.
- Errors cross as `{ name, code, message, param }`.
- The worker file has no logic beyond dispatch.

- [ ] **Step 1: Write the failing tests** (vitest, node environment, a fake
  `Worker` class).
  - `client.test.ts`:
    - `requests are matched by id when replies arrive out of order`;
    - `errors reject with code and param`;
    - `client recovers after a worker crash` (Review Focus 5): the fake
      worker fires `error`. The pending call rejects with
      `code: 'worker_crashed'`, and the next `formats()` call builds a new
      worker and resolves;
    - `outputs arrive as transferable buffers`: assert `postMessage`'s
      transfer list holds each output buffer.
  - `intake.test.ts`:
    - `folder entries keep relative paths`: `a/model.obj`, `a/model.mtl`
      and `a/textures/t.png` become those keys;
    - `a flat list uses file names`;
    - `webkitRelativePath wins when present`.
- [ ] **Step 2: Run RED, implement, run GREEN** (`npx vitest run`). Record.

### Task 12: The converter UI, the samples, the stories

**Files:**
- Create the components:
  `src/components/{DropZone.tsx, SampleGallery.tsx, FormatPicker.tsx, OptionsPanel.tsx, Downloads.tsx, ConverterPage.tsx}`.
- Create the app shell: `src/App.tsx` (replacing the placeholder) and
  `src/App.css`.
- Create the samples: `scripts/make-samples.mjs` and
  `public/samples/<id>/…` plus `public/samples/samples.json` (both
  generated, both committed).
- Stories for each component, plus `ConverterPage.stories.tsx`.

**Interfaces:**
- `samples.json` is `Array<{ id: string; title: string; description: string; format: Format; static: boolean; files: string[] }>`.
  `format` comes from `readModel(...).format`, at generation time.
- `ConverterPage` has no props. It owns one `ConverterClient`. It marks
  itself with these test ids:
  - `drop-zone` and `file-input`;
  - `sample-<id>`;
  - `input-viewer` and `output-viewer`, both `MeshViewer`s;
  - `target-<format>`;
  - `option-<name>`, where name is in the package's spelling;
  - `package-<file|zip|files>`;
  - `losses`, `warnings`;
  - `download`;
  - `error`, showing `code` and the message.
- The UI's behaviour:
  - Choosing input reads it in the worker. The input viewer shows the
    user's own bytes, with `storedFrame` from `formats()` and `up` from
    `fromUp`.
  - Clicking a target, or changing an option, converts.
  - With `zip`, the UI also writes `files` for display. The download is the
    zip.
  - An option control shows only when the option is in the format's
    `readOptions` or `writeOptions`.
  - The package toggle shows only when the target has `writesSidecars`.

- [ ] **Step 1: Generate the samples.** `scripts/make-samples.mjs` imports
  `@litegen/3d-converter` in Node and writes one sample per format:

  | Format | Sample | How |
  |---|---|---|
  | glb | skinned, animated | `fbx/skinned_small_source.glb`, copied |
  | gltf | textured, as gltf + bin + png | `usd/textured_apple.usdz`, converted with `package: "files"` |
  | obj | textured, as obj + mtl + png | `fbx/textured_assimp_7500.fbx`, converted with `"files"` |
  | stl | geometry only | `fbx/cube_assimp_ascii.fbx`, converted |
  | ply | big-endian | `ply/cube_be_struct.ply`, copied |
  | 3mf | assimp, textured | copied |
  | dae | SceneKit, skinned | `skinned_scenekit.dae`, copied |
  | fbx | assimp 7500, textured | copied |
  | usdz | Apple, textured | copied |
  | usda | textured, as usda + png | from the fbx, with `"files"` |

  `static` is true unless `info.animations.length > 0`. The script asserts
  that each `Format` appears exactly once.
- [ ] **Step 2: Write the failing test.** Extend `e2e/stories.spec.ts` with
  `ConverterPage` stories. Each `play` function:
  1. clicks `sample-glb`;
  2. waits for `input-viewer` to be `loaded`;
  3. clicks `target-obj`;
  4. expects `output-viewer` to be `loaded` with `data-format="obj"`.

  Also add `MeshViewer` stories for `obj` (package) and `stl` from the
  samples. Run RED. Record.
- [ ] **Step 3: Implement the components and the page.** The layout and
  look follow the dashboard's palette, through the `mp-*` classes from
  Task 10. Run GREEN. Then `npm run build`, `npx vitest run` and
  `npx playwright test --project stories`. Record.

### Task 13: The e2e runs, the video, the visual proof

**Files:**
- Create: `e2e/{functional.spec.ts, matrix.spec.ts, helpers.ts}`.
- Create: `scripts/{contact-sheet.mjs, videos.mjs}`.
- Modify: `playwright.config.ts`:
  - the projects `e2e`, with the app on `vite preview` 5182 as its
    `webServer`, and `stories`;
  - `viewport 1280×800`;
  - `video: { mode: 'on', size: { width: 1280, height: 800 } }`;
  - `trace: 'retain-on-failure'`.

**Interfaces:**
- `helpers.ts`:
  - `iou(a: string, b: string): number`, over the base64 64×64 masks;
  - `waitLoaded(page, testId, format)`.
- The matrix's targets come from the package's `formats()`, imported in
  Node, so a new format joins without an edit.

- [ ] **Step 1: Write `matrix.spec.ts`.** One `test` per sample in
  `samples.json`, so each sample gets one video. Each test:
  1. clicks `sample-<id>` and waits for `input-viewer` to be `loaded`;
  2. saves the input silhouette;
  3. for each format of `formats()`, clicks `target-<format>`, then
     - waits for `output-viewer` with `data-state="loaded"` and
       `data-format=<format>`;
     - asserts there is no `error` and `data-coverage >= 0.005`;
     - computes the IoU with the input. For a `static` sample it asserts
       IoU ≥ 0.85; otherwise it records the value;
     - screenshots `output-viewer` to
       `e2e-artifacts/shots/<id>-to-<format>-<package>.png`;
     - appends `{ sample, from, to, package, bytes, ms, losses, coverage, iou }`
       to `e2e-artifacts/report.json`;
  4. repeats the format loop with `package-files` for every format with
     `writesSidecars`;
  5. holds 600 ms on each result, so the video shows it.
- [ ] **Step 2: Write `functional.spec.ts`.**
  - `several files make one model`: `setInputFiles` with the gltf sample's
    three files, then renders, converts to glb and renders.
  - `a zip reads as a package`: the obj sample, zipped by the package with
    `package: "zip"`, written to a temporary file, then input.
  - `options apply`:
    - the stl sample with `from-units=mm`: the output glb's input info shows
      the bounds × 0.001;
    - `up=z` on an obj output: `option-up` is visible only for stl, obj and
      ply targets;
    - the fbx colour space is offered only with an fbx end.
  - `zip download`: with `package-zip`, a `download` event fires; its
    `suggestedFilename` ends `.zip` and its bytes start `PK`.
  - `a corrupt file shows its error`: `corrupt.glb` shows `error`, with the
    code `format_mismatch` or `mesh_malformed`, and the page still works
    afterwards.
- [ ] **Step 3: Run it.**
  `npm run build && npx playwright test --project e2e`. All pass. Record
  the pass count. Every failure goes through
  `superpowers:systematic-debugging` before any fix.
- [ ] **Step 4: Produce the proof artifacts.**
  - `node scripts/contact-sheet.mjs` lays every screenshot out in a labelled
    HTML grid, one row per sample and one column per target and package. It
    screenshots that page with Playwright's chromium to
    `e2e-artifacts/contact-sheet.png`.
  - `node scripts/videos.mjs` converts each `test-results/**/video.webm` to
    `e2e-artifacts/videos/<sample>.mp4`
    (`ffmpeg -y -i … -c:v libx264 -pix_fmt yuv420p -movflags +faststart`).
    It then joins them, with the functional runs, into
    `e2e-artifacts/videos/all-conversions.mp4` (the concat demuxer).
  - Check the files exist with `ls -la`, and record the paths.

### Task 14: Parity with the hosted converter

**Files:**
- Create: `packages/3d-converter/scripts/parity.mjs` and
  `packages/3d-converter/scripts/parity_py.py`.

- [ ] **Step 1: Write `parity.mjs`.**
  1. Require `litegen-core/target/release/litegen` to exist, its mtime to be
     newer than every file under `litegen-core/src/mesh/` and `convert.rs`,
     and `grep -rn NEGCTL` to be empty. Otherwise exit 2 with the reason.
     It never builds.
  2. Start the binary on a free port with
     `LITEGEN__DATABASE_URL=sqlite://:memory:`, a master key and
     `LITEGEN_MODELS_DIR=models`, as `dashboard/playwright.config.ts` does.
  3. For every sample (`samples.json`, Bytes input only, so multi-file
     samples are zipped first), every target, and `package` both `file` and
     `zip`: `POST /v1/utils/models3d/convert?from=…&to=…&package=…` with the
     master key.
  4. Compare with the npm package (Node) and with `parity_py.py`, which
     prints the Python output's sha256s. Report each pair as `identical` or
     `differs`, with the first differing offset.
- [ ] **Step 2: Run it.** Expected:
  - Python, native macOS: identical for every pair;
  - wasm: identical for every pair except those that write transcendental
    results (FBX rotations, resampled keys). Each of those differs only
    inside float fields; check a few by hand with the offsets.

  Record the summary. If Python differs anywhere, that's a bug: run
  `superpowers:systematic-debugging`.

### Task 15: Docs, final verification, final review

**Files:**
- Write:
  - `packages/3d-converter/README.md`: the layout, the build commands, how
    to update when mesh grows (core, bindings, stubs, then the app's
    exhaustive switch), Task 7's table, Task 14's results, the
    float-determinism note;
  - `js/README.md`: install, Node, browser, Vite (`worker.format 'es'` and
    `optimizeDeps.exclude`), webpack, the API, the options table, errors,
    limits;
  - `python/README.md`: the same for Python;
  - `apps/3d-converter/README.md`: dev, build, Storybook, the e2e runs,
    hosting under a subpath.
- Modify: the plan's top, with a status block.

- [ ] **Step 1: Write the docs.** Every code sample in a README is also a
  test: the npm README's in `test/readme.test.mjs`, the Python one's through
  `pytest --doctest-glob=README.md`. Run them RED, then GREEN.
- [ ] **Step 2: Final verification, fresh.** Run under
  `superpowers:verification-before-completion`:
  - all core tests and clippy;
  - the JS build and tests;
  - the Python build, plus pytest on 3.10 and 3.14;
  - the app: vitest, build, the stories project, the e2e project;
  - parity.

  Record each `test result:` line, or the pass count, with the tree hash.
- [ ] **Step 3: Final review.** Dispatch one fresh reviewer over the whole
  project against the spec. Fix Critical and Important findings through
  TDD, then rerun Step 2's affected suites.
- [ ] **Step 4: Hand over.** Write the status block at the top of this plan:
  "**Status: done on <date> by session litegen-d0, not committed.**"
  Follow it with what differs from the plan, and the files to add. The
  report to the user ends with one `open "<abs path>"` line per video, the
  contact sheet included, after checking each file exists.
