# 3D converter packages and browser tool: design

Written 2026-11-05 by session litegen-d0. Status: **approved by the user on
2026-11-05**, with the answers in section 11.

## 1. Why

`litegen-core/src/mesh/` converts ten 3D containers (glb, gltf, obj, stl, ply,
3mf, dae, fbx, usdz, usda), every pair, both ways, in-process Rust. It also
reports what a target container can't hold. Today the only way to use it is
litegen's HTTP API (`POST /v1/utils/models3d/convert`). That needs a running
litegen and an API key, and the file has to be uploaded.

Plenty of people want the conversion with no AI generation attached: game and
web developers, AR, 3D printing, pipeline scripts. This project ships the same
library three ways:

- **`@litegen/3d-converter` on npm**: one package for Node.js and browsers,
  built to WebAssembly.
- **`litegen-3d-converter` on PyPI**: a native extension, shipped as abi3
  wheels.
- **A browser tool**, `apps/3d-converter`, to be hosted later at
  `tools.litegen.ai/3d-converter`. You drop a model and see it. The tool
  converts it in the browser, shows the result through that format's own
  loader, and lets you download it. Nothing is uploaded. The tool is built on
  the npm package, so it is the package's first user.

## 2. Goals

- **G1. One implementation.** The packages compile `litegen-core/src/mesh/`
  itself: no fork, no port, no copied rule. The only new Rust is a typed,
  high-level API and the two binding layers.
- **G2. Strict types and docs, generated.** The TypeScript `.d.ts` comes from
  wasm-bindgen and tsify. The Python `.pyi` comes from pyo3-stub-gen. Both
  are generated from the Rust source, doc comments included.
  - Every closed set is a literal union: formats, units, axes, loss kinds,
    error codes. None of them is typed as a plain `string`.
  - A check fails when a generated stub is stale.
- **G3. Familiar names.** The options mirror litegen's HTTP converter and its
  SDKs:
  - TypeScript: `from`, `to`, `units`, `up`, `fromUnits`, `fromUp`,
    `fbxColorSpace`, `package` (as `sdks/typescript`'s `ConvertModel3dOptions`
    has them);
  - Python: `from_format`, `to_format`, `units`, `up`, `from_units`,
    `from_up`, `fbx_color_space`, `package` (as `sdks/python`'s
    `models3d.convert` has them).

  The error codes match the API's too.
- **G4. The hosted converter's results.** The packages run the hosted
  converter's pipeline, apply its option rules, and use its dependency
  versions and features.
  - On one platform, the same input and options give the same bytes. A parity
    run checks this against litegen's own release binary (section 8).
  - Across platforms, a written float can differ in its last bits.
    Transcendental functions come from each platform's libm: Rust's bundled
    port in wasm, Apple's on macOS, glibc's on the server. FBX Euler angles,
    slerp and sRGB use them. Making every platform bit-identical would mean
    changing `mesh/`'s math, which is outside this project. The READMEs say
    so.
- **G5. Best-practice builds, measured.**
  - wasm: wasm-bindgen 0.2.130 with `--target web`, wasm-opt, and size and
    speed measured across opt-level and SIMD choices.
  - Python: PyO3 0.30, maturin 1.16, abi3-py310 wheels, the GIL released for
    the whole conversion, and input borrowed without a copy.
- **G6. As little hand-written JS or Python as possible.**
  - npm: two entry files of about five lines each, and a build script.
  - Python: none; maturin's pure-Rust layout needs none.
- **G7. A browser tool that shows every format**, through an independent
  loader for each, built on the dashboard's 3D viewer design. Its components
  have Storybook stories.
- **G8. Proof.**
  - Test-driven throughout.
  - A Playwright run, recorded on video, that converts every sample (one per
    source format) to every target in the browser and renders each result.

## 3. Non-goals

- **Publishing.** Nothing goes to npm or PyPI, no release CI is set up, and
  there is no domain or hosting work. The user hosts it later. The package
  metadata is ready to publish.
- **Other code.** No edits to litegen's HTTP converter, the dashboard, the
  SDKs, the render harness, or any file under `mesh/`.
- **Moving `mesh/` out of litegen-core.** That move is the full extraction,
  and it would collide with the mesh-gap sessions editing that folder.
  Section 7.2 gets the same effect without moving anything.
- **A native Node addon** (napi-rs). One wasm build serves Node and browsers.
- **New conversion features.** The packages expose what the mesh library has
  and nothing more.
- **A server for the tool.** It is a static site.

## 4. Anti-goals: a failure even if it works

- A second copy of conversion logic, in JS, in Python or in a forked Rust
  copy, that can drift from litegen's.
- Format rules in the app: detecting a format, deciding which options apply,
  picking the main file from several, or computing losses or model facts.
  These all come from the package.
- A preview that renders our own reader's re-reading of our own output, which
  proves nothing. Every output is shown through an independent loader.
- Breaking a peer session: editing a file it holds, building during its
  announced window, or filling the disk.
- Types that say `string` where the set is closed, or docs that exist only in
  a README.

## 5. Constraints

- **`.claude/rules/mesh-standalone.md`.** `mesh/` has to compile as its own
  crate with no code changes.
  - Callers translate at the boundary.
  - No inherent `impl` on a mesh type outside `mesh/`.
  - No foreign trait impl for a mesh type (the orphan rule).

  So the bindings define their own types and convert.
- **The shared tree** (`.claude/rules/working-in-this-repo.md`):
  - no worktrees, no commits, nothing staged;
  - edits only in this project's new folders, its spec, plan and ledger;
  - cargo only when no peer runs it: wait with `pgrep -x`, grep for
    `NEGCTL` first, and hold during announced windows (litegen-b2's item 4
    has one open now);
  - `CARGO_INCREMENTAL=0`; check `df` before builds and stop below 8 GiB.
- **The toolchain.** Homebrew's Rust 1.98.1 ships no wasm32 standard library.
  The build needs:
  - Homebrew's `rustup`, which is keg-only: it doesn't replace the
    `/opt/homebrew/bin/cargo` peers use. Then toolchain 1.98.1 with the
    `wasm32-unknown-unknown` target, pinned by a `rust-toolchain.toml` in
    `packages/3d-converter/`;
  - `wasm-bindgen-cli` 0.2.130, which must equal the crate's version. Install
    it with `cargo install --locked` into `packages/3d-converter/.tools/`
    (git-ignored), so a later `brew upgrade` can't break the build;
  - binaryen's `wasm-opt`;
  - maturin through `uvx`, with no global install;
  - Python 3.10 through `uv`, for the abi3 floor test.
- **Disk.** 46 GiB free. The toolchain and this project's own target folder
  take about 5–8 GB.
- **`mesh/` grows while this is built.** Per litegen-fc, items 2–10 add:
  - `MeshFormat::Usdc`;
  - `WriteOptions` fields `usdz_root`, `usd_clips`, `fbx_encoding` and
    `dae_morphs`;
  - the crates `draco-core =2.3.3` and `ruzstd =0.9.1`.

  Section 7.3 says how the packages keep up.

## 6. The repo's "SDKs and clients stay thin" rule

litegen-fc asked that the spec address this rule explicitly, so the user can
decide it. The rule says the SDKs and the dashboard wrap requests, poll and
give typed shapes. They do no 3D parsing and copy no server rule.

**My reading.** The packages are not a client copying a server rule. They are
the server's own mesh library, compiled for another runtime: same source, same
tests. So they sit outside the rule's scope, kept apart from `sdks/` and
`dashboard/`. The app, by contrast, is a client of the npm package, and the
rule applies to it in full:

- It holds no format rules. It asks the package for:
  - the detected format;
  - the main file among several dropped files;
  - which options apply to a pair;
  - what each format keeps;
  - the losses;
  - the model's facts (triangles, bounds, materials, skin, clips).
- It also asks the package for the facts its viewer needs:
  - the frame a file stores, for example 3MF's +Z up;
  - each file's role: model, material library, buffer or texture.

  So it parses no file to find the `.mtl` and knows no format's axis.
- Its only 3D code renders, through three.js's loaders. That is the engine
  behind the dashboard's `<model-viewer>`. The viewer code is generic: it
  maps files to their loader, serves sidecars from memory and frames the
  camera.

If the user reads the rule differently, the fallback is to drop the app's own
rendering of non-glTF formats. Non-glTF outputs would then be shown only by
converting them back to GLB with our own reader. That costs the independent
proof in G7 and G8.

## 7. Architecture

### 7.1 Layout

```
packages/3d-converter/            # one Cargo workspace, its own target/ (git-ignored)
  Cargo.toml  Cargo.lock  rust-toolchain.toml  README.md  .gitignore
  mesh/Cargo.toml                 # crate litegen-mesh: litegen-core/src/mesh/ as a crate
  core/                           # crate litegen-3d-converter: the typed high-level API
  js/                             # crate litegen-3d-converter-js + npm @litegen/3d-converter
    Cargo.toml  src/lib.rs  package.json  README.md  entry/{node,web}.js  scripts/build.mjs
    test/                         # node:test on the built package, plus a strict tsc type test
  python/                         # crate litegen-3d-converter-py + PyPI litegen-3d-converter
    Cargo.toml  src/lib.rs  src/bin/stub_gen.rs  pyproject.toml  README.md
    litegen_3d_converter.pyi      # generated, checked current
    tests/                        # pytest, stubtest, typed-usage check
apps/3d-converter/                # Vite + React app, Storybook, Playwright e2e
```

`packages/` pairs with the existing `apps/`. Each new folder has its own
`.gitignore`, so the shared root `.gitignore` is left alone.

### 7.2 `litegen-mesh`: the mesh folder as a crate, with nothing moved

`packages/3d-converter/mesh/Cargo.toml` is the whole crate:

- `[lib] path = "../../../litegen-core/src/mesh/mod.rs"`;
- `edition = "2021"`, litegen-core's edition;
- the same dependency specs litegen-core gives `serde_json`, `miniz_oxide`,
  `quick-xml` and `image`. For `image` that means
  `default-features = false, features = ["png", "gif", "webp", "jpeg"]`.

Two checks keep it in step:

- `deps_match_litegen_core` fails when a spec drifts from litegen-core's.
- A second test fails when a resolved version in this workspace's `Cargo.lock`
  differs from litegen-core's. Same versions and features mean the same bytes
  out (G4).

When items 2 and 3 add `draco-core` and `ruzstd`, this manifest adds them.
The tests then force the same specs.

Compiled as a crate root, `mesh/` is held to the standalone rule more
strictly than `isolation_tests.rs` holds it. An escaping `super::` or an
undeclared crate fails this build. If that ever happens, I tell the session
that made the edit; I don't work around it.

This crate can't be published to crates.io, because its source lies outside
the package folder. Nothing needs that: the wheels and the npm tarball carry
compiled output. The later extraction (section 10) makes it an ordinary crate.

### 7.3 `litegen-3d-converter`: the one high-level API

Plain Rust with no binding dependencies. Both bindings wrap it, so they can't
disagree.

**The pipeline.** It is the HTTP handler's `convert_bytes`
(`api/handlers/convert.rs`), call for call:

1. `read_scene_with`;
2. `normalize::read_frame`, for `from_units` and `from_up`;
3. `normalize::transcode_textures`;
4. `normalize::express_frame`, for `units` and `up`;
5. `write_scene`;
6. each texture that couldn't be decoded becomes an `image_formats` loss;
7. with `package = zip`, `zip::write_package`.

`Model` runs steps 1–2 once and keeps that scene. Step 3 runs on first need
and is cached, so the untranscoded scene stays available. Item 3's planned
`ktx2_textures=keep` writes glb and gltf from it. Steps 4–7 run per write. A
test checks that this gives exactly the bytes of a one-shot `convert`.

**Types.**

- `Format` (10 values), `Packaging` (`file`, `zip`, `files`),
  `Unit` (`m`, `cm`, `mm`, `in`, `ft`), `UpAxis` (`y`, `z`),
  `FbxColorSpace` (`srgb`, `linear`).
- `ReadOptions`: `from`, `file_name`, `from_units`, `from_up`,
  `fbx_color_space`.
- `WriteOptions`: `to`, `package`, `units`, `up`, `fbx_color_space`, `fps`.
- `Input`: one file's bytes, or a set of named files.
- `Model`: holds the read scene. `read`, `info`, `write`, `format`,
  `warnings`.
- `ModelInfo`: from `inspect::inspect`, plus node, mesh and image counts:
  - triangles, vertices, uv sets, vertex colour;
  - bounds in metres;
  - materials and their texture maps;
  - the skin: joints, root, bind pose, humanoid map;
  - the clips: duration, root travel, in place.
- `Conversion`: `format`, `files`, `losses`, `warnings`.
- `OutputFile`: `name`, `media_type`, `role`, `data`. `role` is `model`,
  `material_library`, `buffer` or `texture`. The writer knows which is which,
  so the app never parses a file to find the `.mtl`.
- `Model` also lists the input's files with the same roles: the main file and
  each sidecar the reader asked for, recorded by the core's sidecar resolver.
  Any other dropped files are listed as unused.
- `Loss`: `kind` (15 `LossKind`s), `what`, `why`.
- `FormatInfo`, all from the Rust side:
  - name, label, extensions, media type;
  - whether it writes sidecars;
  - what it keeps (`Capabilities`), as one file and as a package;
  - which options it takes, read side and write side;
  - its frame facts: whether the container declares its own units and up
    axis, and the frame it stores (3MF: +Z up; the others written as glTF's
    +Y up unless `up = z`).

  So the viewer applies one generic rule ("a +Z-up file turns −90° about X")
  and holds no 3MF fact of its own.
- `Error`: `code`, `message`, `param`.

**Functions.** `convert`, `Model::read`, `Model::write`, `detect_format`,
`formats`, and `MAX_INPUT_BYTES` (`mesh::MAX_INPUT_BYTES`).

**Format detection.** By magic bytes first, then file name:

| Clue | Format |
|---|---|
| `glTF` | glb |
| `Kaydara FBX Binary`, or ASCII `; FBX` | fbx |
| `PXR-USDC`, `#usda` | usda |
| `ply` | ply |
| a zip holding `3D/3dmodel.model` | 3mf |
| a zip whose first entry is a USD layer | usdz |
| `<COLLADA` | dae |
| glTF JSON (`"asset"`) | gltf |
| a binary STL whose size matches its count, or ASCII `solid … facet` | stl |
| OBJ `v` and `f` lines | obj |

Then the name: `.usdc` and `.usd` read as `usda`, as the HTTP API documents.
When nothing matches, the error is `format_unrecognized`, and the caller can
pass `from`.

**Several files in.**

- The main file is the one with a model extension. If several have one, it is
  the one matching `from`. If two still match, the error is
  `input_ambiguous`.
- Every other file is a sidecar.
- A zip that isn't a 3MF or USDZ is unpacked and handled the same way. So
  litegen's own `package=zip` output reads back.
- The sidecar resolver tries the exact relative path first, then a unique
  match on the file name. Browser drops of loose files arrive without their
  folders.

**Option rules.** The HTTP API's, so the same mistake fails the same way:

| Option | Taken only when | Otherwise |
|---|---|---|
| `units`, `up` | the target is stl, obj or ply | `param_unsupported` |
| `from_units`, `from_up` | the source is stl, obj or ply | `param_unsupported` |
| `fbx_color_space` | an end is fbx (in `convert`; on `read` the source, on `write` the target) | `param_unsupported` |
| `fps` | the target resamples animation: fbx, dae, usdz, usda | `param_unsupported` |

An out-of-range value is `param_invalid`. `fps` exists in `mesh`'s
`WriteOptions` but the HTTP API doesn't expose it; the packages offer it as
an opt-in.

**Error codes.**

- From the API: `format_mismatch` (`MeshError::NotThisFormat`),
  `mesh_malformed`, `mesh_unsupported`, `mesh_empty`, `param_unsupported`,
  `param_invalid`.
- New: `input_too_large` (the API calls it `body_too_large`),
  `format_unrecognized`, `input_ambiguous`.

**Keeping up with `mesh`.** The mappings from `MeshFormat` and `LossKind`
are exhaustive `match`es. `mesh::WriteOptions` is built with a full struct
literal, never `..Default::default()`. So a new format, loss kind or write
option fails the build at the exact line to update. The failure happens on my
crates only: peers' builds never compile them. The README says what to update
and in which order (core, the bindings, then the stubs).

**Keeping up with the HTTP handler.** The pipeline and option rules above
copy code that lives outside `mesh/`: `convert_bytes`, `parse_query` and
`PARAMS` in `api/handlers/convert.rs`. Exhaustive matches can't see a change
there. A tripwire test catches it instead.

- It pins a hash of those three items' source text and fails when any of
  them changes.
- The failure message names the core functions to re-check against the
  change.
- After re-checking them, you update the pin.

This duplication ends with the extraction in section 10.

**What the packages cover.** They cover the converter as it is on the day each
task is built:

- 10 formats;
- the HTTP API's 8 parameters;
- `fps`.

A mesh-gap item that lands before this project ends is folded in. Each is an
opt-in option whose default keeps today's output, so adding one is additive:
a field in the core, then in each binding, then the stub. Item 6's `usdc`
format and options, item 7's `fbx_encoding` and item 8's `dae_morphs` are
the ones expected. None of this project's tasks waits on another session.

**Media types.** A copy of litegen's table (`proxy/storage.rs`,
`model3d_content_type`), with fbx and ply as `application/octet-stream` as
the API sends them. A test pins each value.

### 7.4 The npm package `@litegen/3d-converter`

**The crate.** `litegen-3d-converter-js` is a `cdylib` built on:

- `wasm-bindgen =0.2.130`;
- `tsify 0.5.8`, using its `Ts<T>` parameters (its README deprecates the ABI
  attributes);
- `serde-wasm-bindgen 0.6.5`;
- `serde_bytes`, so byte fields cross as `Uint8Array`;
- `js-sys`.

**The API, as the generated `.d.ts` will read.** Every field carries its Rust
doc comment as JSDoc.

```ts
export type Format = "glb" | "gltf" | "obj" | "stl" | "ply" | "3mf" | "dae" | "fbx" | "usdz" | "usda";
export type ModelInput = Uint8Array | Record<string, Uint8Array>; // one file, or named files

export function convert(input: ModelInput, options: ConvertOptions): Conversion;
export function readModel(input: ModelInput, options?: ReadOptions): Model;
export function detectFormat(input: Uint8Array, fileName?: string): Format | undefined;
export function formats(): FormatInfo[];

export interface ReadOptions { from?: Format; fileName?: string; fromUnits?: Unit; fromUp?: UpAxis; fbxColorSpace?: FbxColorSpace }
export interface WriteOptions { to: Format; package?: Packaging; units?: Unit; up?: UpAxis; fbxColorSpace?: FbxColorSpace; fps?: number }
export interface ConvertOptions extends ReadOptions, WriteOptions {}
export interface Conversion { format: Format; files: OutputFile[]; losses: Loss[]; warnings: string[] }
export interface OutputFile { name: string; mediaType: string; data: Uint8Array }
export interface Loss { kind: LossKind; what: string; why: string }

export class Model {           // read once, write many: the scene stays in wasm memory
  readonly format: Format;
  readonly info: ModelInfo;
  readonly warnings: string[];
  write(options: WriteOptions): Conversion;
  free(): void;                 // Symbol.dispose too, if wasm-bindgen emits it (checked)
}
export class ConvertError extends Error { readonly code: ErrorCode; readonly param?: string }
```

**Usage.**

```ts
import { convert, readModel } from "@litegen/3d-converter";

const { files, losses } = convert(glbBytes, { to: "usdz" });
const m = readModel({ "fox.gltf": gltf, "fox.bin": bin, "fox.png": png });
const obj = m.write({ to: "obj", package: "zip" }); // OBJ + MTL + textures in one zip
m.free();
```

**`ConvertError`** is a real `Error` subclass, so `instanceof` works. It is
declared in a few lines of `#[wasm_bindgen(inline_js)]` inside the Rust
source.

**Shape.** The package is ESM-only and supports Node 22 and later. It is
wasm-bindgen's `--target web` output plus two hand-written entries:

- `node.js` reads the `.wasm` from disk and calls `initSync`, so Node needs
  no top-level await.
- `web.js` awaits `init()` at the top level, with
  `new URL("…_bg.wasm", import.meta.url)`. Vite, webpack, Rollup and esbuild
  all emit that URL as an asset.

`package.json` routes them with conditional exports: `types`, `node`,
`default`. So `import { convert }` just works in both places, with the same
types.

wasm-bindgen's `module` target would need no Node file, but it works today
only in Node 24 and esbuild, not in Vite or browsers.

**Build.** `npm run build` runs `scripts/build.mjs`:

1. cargo, through rustup's toolchain;
2. `wasm-bindgen`;
3. `wasm-opt`;
4. copy the entries;
5. print the raw, gzip and brotli sizes.

**Optimization, chosen by measurement.** The `release-wasm` profile has
`lto = "fat"`, `codegen-units = 1` and `panic = "abort"` (wasm32 can't
unwind). The variables are opt-level (`3`, `s`, `z`), `wasm-opt` (`-O3` or
`-Oz`) and `+simd128` (on or off).

- I measure each variant's raw, gzip and brotli size, and its time on a
  Node benchmark that runs fixed fixture conversions.
- The rule: take the fastest variant, unless another is within 5% of its
  speed and at least 15% smaller.
- The numbers go in the README.
- The allocator stays Rust's default dlmalloc. `wee_alloc` is unmaintained.

### 7.5 The Python package `litegen-3d-converter`

**The crate.** `litegen-3d-converter-py` is a `cdylib` plus an `rlib`, the
`rlib` for the stub generator. It uses `pyo3 0.30.1` with `abi3-py310`, and
`pyo3-stub-gen 0.23.2` behind a `stubgen` feature, so that tool stays out of
the wheel.

**The packaging.** A `pyproject.toml` with:

- the maturin 1.16 backend;
- `module-name = "litegen_3d_converter"`, in maturin's pure-Rust layout;
- `requires-python = ">=3.10"`;
- `Typing :: Typed`.

**The API, as the `.pyi` will read.**

```python
Format = Literal["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda"]
ModelInput = bytes | bytearray | memoryview | Mapping[str, bytes]

def convert(data: ModelInput, *, to_format: Format, from_format: Format | None = None,
            file_name: str | None = None, package: Packaging = "file",
            units: Unit | None = None, up: UpAxis | None = None,
            from_units: Unit | None = None, from_up: UpAxis | None = None,
            fbx_color_space: FbxColorSpace | None = None, fps: int | None = None) -> Conversion: ...
def read_model(data: ModelInput, *, from_format: Format | None = None, ...) -> Model: ...
def detect_format(data: bytes, file_name: str | None = None) -> Format | None: ...
def formats() -> list[FormatInfo]: ...

class Model:            # frozen; read once, write many
    @property
    def format(self) -> Format: ...
    @property
    def info(self) -> ModelInfo: ...
    def write(self, *, to_format: Format, package: Packaging = "file", ...) -> Conversion: ...

class ConvertError(ValueError):
    code: ErrorCode
    param: str | None
```

`Conversion`, `OutputFile` (`data: bytes`), `Loss`, `ModelInfo` and
`FormatInfo` are frozen classes with typed read-only properties. Every closed
set is a `Literal` alias.

**Performance.**

- Input is borrowed with no copy where Python allows: `bytes` through
  `PyBackedBytes`, and other buffers through the buffer protocol.
- The GIL is released for the whole read, convert or write (`py.detach`).
- `#[pymodule(gil_used = false)]`, so a free-threaded build doesn't turn the
  GIL back on.
- Panics stay unwinding and arrive as PyO3's `PanicException`, not as a
  crashed interpreter.

**Stubs.** `cargo run --bin stub_gen --features stubgen` writes
`litegen_3d_converter.pyi`, and maturin ships it with `py.typed`. Three
checks:

- a test regenerates the stub and diffs it;
- mypy's `stubtest` checks it against the runtime module;
- mypy and pyright, in strict mode, check a typed usage file, including lines
  that must fail.

### 7.6 The browser tool, `apps/3d-converter`

**Stack.** Vite 8, React 19, TypeScript ~6.0 (the dashboard's), three 0.183.2
(the harness's, whose loader behaviour it has proven), Storybook 10.6,
Vitest 4, and Playwright at the version whose browsers are already installed.

- It depends on `"@litegen/3d-converter": "file:../../packages/3d-converter/js"`.
- It is a static site with `base: './'`, so it works under any path,
  `tools.litegen.ai/3d-converter` included. It has no router.

**What it does.**

1. **Input.**
   - Drop files or a folder, or pick them.
   - Several files are one model plus its sidecars, and a zip is unpacked.
   - A sample gallery offers one model per source format.
2. **Read.** In a module Web Worker. The tool shows the input through its own
   format's loader, plus the model's facts and the reader's warnings.
3. **Target.** Ten format buttons, each saying what that format keeps. That
   comes from `formats()`.
4. **Options.** Shown only where `formats()` says they apply: packaging,
   units and up axis (target and source), FBX colour space, frame rate.
5. **Convert.** In the worker. Outputs come back as transferable buffers, with
   no copy. A wasm trap restarts the worker.
6. **Output.**
   - The converted file through its own format's loader.
   - Its losses (kind, what, why), its warnings, size, time and files.
   - A download: the file, the zip, or each file.

**The viewer.** `MeshViewer` is three.js, with each format's own loader:

| Format | three.js loader |
|---|---|
| glb, gltf | `GLTFLoader` |
| obj | `OBJLoader`, plus `MTLLoader` for the material library |
| stl | `STLLoader` |
| ply | `PLYLoader` |
| 3mf | `ThreeMFLoader` |
| dae | `ColladaLoader` |
| fbx | `FBXLoader` |
| usdz, usda | `USDLoader` |

- Sidecars are served from memory through `LoadingManager.setURLModifier`.
- Auto-framing, animation playback, exposure, three backgrounds, reset camera
  and fullscreen.
- Per-loader conventions are ported from `scripts/mesh-verify/page/render.js`:
  orientation, V direction, units. The harness has proven them against every
  format.

**Reusing the dashboard's 3D components.** The dashboard's viewer is
`<model-viewer>`, which renders glTF and GLB only. It also gets its file
through `useFileSrc`, which imports the dashboard's authenticated API client.
So it can't show eight of the ten formats, and it can't be imported without
pulling in the API SDK. Instead the tool ports what transfers, and doesn't
edit the dashboard:

- from `ModelViewer.tsx`:
  - its prop and handle shape: `src` becomes `files`, plus `format`,
    `autoRotate`, `exposure`, `background`, `onLoad` (with dimensions),
    `onError`, `resetCamera`;
  - its loading, progress and error fallback;
  - its WebGL 2 probe;
  - its camera framing values and backgrounds;
- from `ModelPreview.tsx` and its CSS: the inspector toolbar, the stats grid,
  the scene facts and clips table, and its look (the palette and the `mp-*`
  classes);
- the dashboard's Storybook setup and fixtures.

Each ported file names its source. The dashboard could later adopt
`MeshViewer` to preview the formats it now offers only as downloads; that's a
follow-up.

### 7.7 Storybook

The app gets its own Storybook, set up like the dashboard's: react-vite, no
addons, telemetry off.

- **Stories:**
  - `MeshViewer`, one story per sample format, plus the loading, error and
    no-WebGL states;
  - `DropZone`, `FormatPicker`, `OptionsPanel`, `LossList`, `ModelFacts`,
    `Inspector`;
  - the whole app.
- **A Playwright smoke test** opens every story in the built Storybook. It
  fails on any console error, and on an empty canvas in a viewer story.

## 8. Verification

Every behaviour change goes RED, then GREEN. The ledger records each step:
`.superpowers/sdd/2026-11-05-3d-converter-packages/progress.md`.

- **Rust (core):**
  - detection for every format, by magic and by name;
  - the default `convert` equals the mesh pipeline's bytes;
  - read-once-write-many equals a one-shot `convert`;
  - each package mode (one file; zip entries; files, main file first);
  - every option rule and error code;
  - units and up-axis effects, read back as bounds;
  - FBX colour space and fps reach the writer;
  - several-file input: main-file choice, sidecars, the name fallback, zip
    input, ambiguity;
  - the size cap;
  - `formats()` against `MeshFormat::capabilities`;
  - `ModelInfo` against fixtures;
  - the dependency-drift tests.
- **JS package:**
  - `node:test` on the built tarball: every format pair from Node, error
    classes and codes, `Model`, detection;
  - a strict `tsc` type test, where `@ts-expect-error` lines prove bad
    options don't compile;
  - an `npm pack --dry-run` file list and a size budget;
  - the browser entry, through the app's Playwright run.
- **Python:**
  - pytest on the installed wheel under Python 3.10 and 3.14;
  - a test that the GIL is released during a conversion;
  - stubtest, the stub-current check, and mypy and pyright strict;
  - the wheel's tag is `abi3`.
- **App:** Vitest for the worker protocol and the file-drop mapping, the
  Storybook smoke test, and the e2e run below.
- **Parity with the hosted converter.** A script starts litegen's release
  binary and runs every sample through every target, in both package modes,
  over `POST /v1/utils/models3d/convert`. It compares the bytes with the
  packages' own output.
  - It never builds litegen. It uses `litegen-core/target/release/litegen`
    as peers last built it, and only when the binary is newer than every
    file in `mesh/`.
  - With the macOS wheel (same platform, same libm) it expects identical
    bytes.
  - With the wasm build it expects identical bytes wherever no
    transcendental function is involved. It reports each pair that differs,
    and confirms the difference is only in a float's last bits.

**The e2e run on video.**

- **The samples** are one per source format, each a different kind of
  model:
  - textured, skinned and animated, vertex-coloured, multi-file;
  - fbx, dae, 3mf, usdz and ply files written by independent tools (assimp,
    SceneKit, Apple), taken from `mesh/testdata`;
  - our own writer's output from `mesh/fixtures.rs` for the rest;
  - all of it litegen's own content: the fixture READMEs say no third-party
    content is involved.
- **The matrix:**
  - every sample to every target `formats()` lists, as one file;
  - plus package mode for every target that writes sidecars;
  - all in the browser, through the UI.

  Today that's 10 × 10, plus 10 × 4 for obj, gltf, dae and usda: 140
  conversions. The run reads the lists from the package, so a new format such
  as `usdc` joins without a test edit.
- **Each output:**
  - is rendered by its format's three.js loader;
  - must load with no error and show visible pixels (coverage at or above a
    threshold);
  - gets its silhouette's overlap with the input's rest-pose silhouette
    recorded, which must reach a set floor on static samples;
  - is screenshotted.

  The run also writes a contact sheet of all 140 and a JSON report (bytes,
  milliseconds, losses).
- **Functional runs too:** a several-file drop (gltf + bin + png, and
  obj + mtl + png), a zip input, the options (units, up axis, FBX colour
  space, a zip download), and a corrupt file.
- **The video.** Playwright records one video per sample. ffmpeg turns them
  into MP4 and joins them into one. The handover ends with `open` commands for
  the videos.

## 9. Decisions I made

These are my calls, not the user's. The user can overturn any of them.

1. **Mirror the HTTP API's and SDKs' names and error codes**, adding only
   `format_unrecognized`, `input_ambiguous`, `input_too_large` and the
   `files` packaging. A move from the hosted API to the package then changes
   almost nothing.
2. **`from` is optional in the packages.** Detection runs in Rust when it's
   absent. The hosted API requires `from`.
3. **Strict option rules**, the API's: a misapplied option is an error, not
   silently ignored.
4. **Flat options, not a discriminated union.** A union could make
   `units` on a glb target a compile error, but it would read differently
   from the SDK's options. The rule is checked at run time instead, with a
   clear `param` in the error.
5. **wasm only for npm, one package.** No native Node addon.
6. **Native classes and generated `.pyi` stubs for Python**, not pydantic.
   pydantic would add a dependency and runtime validation the Rust side
   already does. Literal types and stubtest give the same editor completion.
7. **Python ≥ 3.10.** That is pyo3-stub-gen's floor, with one abi3 wheel per
   platform. Node ≥ 22: Node 20 reached end of life in April 2026.
8. **Names.** npm `@litegen/3d-converter` (the user's); PyPI
   `litegen-3d-converter`, imported as `litegen_3d_converter`.
9. **`packages/3d-converter/`** holds the workspace, with its own target
   folder, so peers' builds never compile it and its builds never take
   litegen-core's cargo lock.
10. **The viewer is ported, not imported** (section 7.6). The dashboard is
    left untouched.
11. **three 0.183.2**, matching the harness, rather than the latest 0.187.0.
12. **"Each and every model variation"** means every sample (one per source
    format, each a different kind of model) times every target, in both
    package modes where they differ: 140 conversions today, more as formats
    are added.
13. **Track the converter as it is when built, and fold in mesh-gap items
    that land before the end.** No task waits on another session. A tripwire
    test guards the copied handler logic (section 7.3).
14. **Bytes identical on one platform, not across platforms** (G4). Exact
    identity everywhere would need a change to `mesh/`'s math.
15. **Facts the viewer needs come from the package**: the frame a format
    stores, and each file's role. The app parses no file to find a
    material library, and knows no format's axis.

## 10. Follow-ups, not in this project

- Move `mesh/` into `packages/3d-converter/mesh/` once the mesh-gap items
  finish. litegen-core then depends on the crate, and its HTTP handler calls
  the core crate, so the pipeline exists once.
- Release CI:
  - maturin-action wheels for manylinux and musllinux (x86_64 and aarch64),
    macOS and Windows;
  - npm publish with provenance;
  - free-threaded wheels (`abi3t-py315`) once Python 3.15 is the floor for
    them.
- The dashboard adopts `MeshViewer`.
- A napi-rs native Node build, if Node's wasm speed ever matters.

## 11. Questions for the user

A builder read this document cold and raised three questions. Two I settled
(decisions 13 to 15, and G4); the third is the rule in section 6. The user's
answers to these two are recorded here:

1. **The dashboard viewer:** port its design into the app and leave the
   dashboard untouched, or move a shared viewer into a package the dashboard
   also switches to. — **The user chose: port it, dashboard untouched**
   (2026-11-05).
2. **The thin-client rule (section 6):** accept my reading, or the strict
   one. — **The user chose: my reading** (2026-11-05).

The user also approved the build and its tool installs: Homebrew's `rustup`
(keg-only), the wasm32 target, `wasm-bindgen-cli` 0.2.130 in a git-ignored
local folder, and binaryen.
