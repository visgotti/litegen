# 3D converter packages: rulings and leftovers

The decisions made on the user's behalf while building the npm and PyPI
packages of litegen's 3D converter and its browser tool (2026-11-05). They
are copied from the work ledger, `.superpowers/sdd/2026-11-05-3d-converter-packages/progress.md`,
which git ignores. Each ruling ends with what it costs if it is wrong.
Nothing here is committed; the user commits on their own go.

Spec: `docs/superpowers/specs/2026-11-05-3d-converter-packages-design.md`.
Plan: `docs/superpowers/plans/2026-11-05-3d-converter-packages.md`.

## What was built

- `packages/3d-converter/`: a Cargo workspace. It compiles `litegen-core/src/mesh/` as a crate,
  with nothing moved. On top of that crate it has the typed core API and two bindings:
  - `@litegen/3d-converter` on npm: wasm-bindgen with tsify, for Node and browsers;
  - `litegen-3d-converter` on PyPI: PyO3, as one abi3 wheel for Python 3.10 and later.
- `apps/3d-converter/`: a static browser tool on the npm package, with Storybook stories and a
  recorded Playwright e2e run. It is not part of the Docker image.
- Since 2026-11-06 the tool's viewer and inspector have lived in `packages/3d-viewer`
  (`@litegen/3d-viewer`), and the dashboard imports them too, in place of `<model-viewer>`.
  Design: `docs/superpowers/specs/2026-11-06-shared-3d-preview-design.md`.
- Verified on 2026-11-05, against mesh tree 6879167 (litegen-b2's item 4, settled):
  - core 68/68; clippy clean;
  - js 24/24, plus three tsc projects;
  - pytest 36/36 on Python 3.14 and on 3.10;
  - vitest 44/44; stories 35/35; e2e 18/18 (140 conversions, all drawn).
- Parity with litegen's HTTP converter:
  - Python: 180/180 byte-identical;
  - npm: 174/180. The six that differ are FBX writes that differ in one to four doubles by one
    unit in the last place, because WebAssembly has its own maths library.

## Rulings

### Setup and the core

- The handler tripwire and the mesh dependency-drift tests are deliberate change detectors —
  cost if wrong: a litegen change fails a test until it is re-checked.
- `Cargo.lock` is seeded from litegen-core's, pruned to mesh's packages — cost if wrong: none;
  a test re-checks the versions on every run.
- `the_input_cap_is_meshs` was removed, since it only detected changes — cost if wrong: none;
  the size-cap tests cover the cap.
- Error messages are kept in parts, so each binding spells option names its own way
  (`to` and `to_format`) — cost if wrong: one accessor rename in the bindings.
- The mesh crate gets exactly litegen-core's seven clippy allows for `mod mesh` — cost if wrong:
  an allow that litegen-core adds later fails this clippy until it is copied.
- A zip is judged by its entries (3MF, USDZ, or a package to unpack), never by its name —
  cost if wrong: a mislabelled zip gets `format_unrecognized` instead of `format_mismatch`.
- `file_name` also picks the main file among several files — cost if wrong: one more option
  field to rename.
- Finder's own files (`__MACOSX/`, `._*`, `.DS_Store`) are skipped in zips — cost if wrong:
  a model really named `._x.obj` cannot be read from a zip.
- Textures are transcoded on a model's first write and cached, and the untranscoded scene is
  kept — cost if wrong: one extra scene copy on that first write. One-shot `convert` transcodes
  and frames in place, with no copy.
- `convert` takes the FBX colour space from the read options, or else the write options, and
  applies it at both ends, as the HTTP API does — cost if wrong: none for the bindings, which
  set one value.
- Input files are a `Cow` (borrowed or owned) — cost if wrong: none.
- `Model.info` is computed lazily and can fail. Reads, writes and `convert` never compute it,
  as the hosted converter never does — cost if wrong: none.
- mesh's humanoid naming schemes and bones are typed. A test reads `mesh/humanoid.rs`, so a
  scheme or bone that mesh adds fails the suite until it is typed — cost if wrong: a change to
  that file's layout breaks the test, not the build.
- Humanoid bones are a list of `{bone, joint}` in VRM order, the same in TypeScript and Python —
  cost if wrong: a caller who wants a map builds one in one line.
- File names are compared in Unicode's composed form (NFC), and the fallback that matches by
  file name ignores case beyond ASCII. `unicode-normalization` 0.1 joins the core crate, not
  mesh — cost if wrong: +52 KiB brotli in the wasm (938 KiB; the budget is 970).

### The npm package

- The type test is named `types.check.ts`, and the test runner only takes `*.test.mjs`
  (Node 26 strips types and would run the compile-only file) — cost if wrong: none.
- Unknown option keys are found by serde introspection rather than a hand-written list —
  cost if wrong: none.
- The init declarations are stripped from the `.d.ts`, because both entries instantiate the
  wasm and those types name DOM-only types — cost if wrong: `initSync` has no type.
- The panic hook installs on first use. `detectFormat`'s parameter is named `fileName` and typed
  `Uint8Array` — cost if wrong: none.
- `entry/model.js` wraps wasm-bindgen's `Model`: freeing it twice is a no-op, and a freed
  model's methods throw a `TypeError` that says so — cost if wrong: it reads wasm-bindgen's
  internal `__wbg_ptr`, pinned by the exact version (0.2.130).
- Closed sets keep their exact spellings (`+z` and `GLB` are refused), so the runtime matches
  the literal types — cost if wrong: a JS caller passing `+z` gets `param_invalid` naming
  `y` and `z`.

### The PyPI package

- Closed-set arguments are typed newtypes, a `str` at runtime — cost if wrong: none; stubtest
  checks the result.
- The `Literal` aliases are built from the core's value lists, in the stub and at runtime —
  cost if wrong: none.
- `package` defaults to `None`, meaning "file" — cost if wrong: an IDE shows `None` rather
  than `'file'`.
- The stubtest allow-list holds the 16 `Literal` aliases (a stubtest limitation, reproduced
  in pure Python) and `BytesLike` (PEP 688's `Buffer` in the stub, the built-in buffer union at
  runtime) — cost if wrong: a real mismatch under those names goes unreported, though a
  runtime test checks the alias values.
- Only `bytes` is read in place. Any other buffer is copied once, by `memoryview.tobytes()`,
  after the size cap is checked: another thread may write it while the GIL is released, and
  the buffer protocol is outside abi3-py310 — cost if wrong: one copy of a non-bytes input.
- `__version__` is added to the stub's `__all__` by `stub_gen` — cost if wrong: none.
- Results stay unpicklable, which the README documents; a `ConvertError` built in user code
  has no `code` — cost if wrong: a caller who pickles results copies the fields.
- `pyo3/extension-module` stays, since PyO3 0.30 still defines it — cost if wrong: one line
  to drop later.
- The cross-binding naming differences stay (JS `duration`/`rootTravel` and
  `maxInputBytes()`; Python `duration_s`/`root_travel_m` and `MAX_INPUT_BYTES`), as each
  binding's idiom — cost if wrong: a rename in one binding later.

### The browser tool

- The story test ran red against a stub viewer only once Playwright was free (a peer held it)
  — cost if wrong: none.
- Option test IDs use the TypeScript spelling, and the loss list is `output-losses` —
  cost if wrong: renaming test IDs.
- A zip output is shown through a second write of the same model as separate files —
  cost if wrong: one extra write per zip conversion.
- Each format's three.js loader is its own chunk — cost if wrong: one fetch the first time a
  format is shown.
- The viewer's load state is keyed by its files — cost if wrong: none.
- The USDZ sample's shapes are compared with its glb conversion, and the inspector says when a
  loader drew fewer triangles than the converter read — cost if wrong: a loader regression on
  that one input view goes unmeasured by shape.
- When several files could be the model, the worker attaches the package's own `detectFormat`
  matches, and the page asks which one it is — cost if wrong: a non-model file the package
  recognises is offered too.
- The viewer puts a text USD layer's skeleton rotations back in three.js's order, because
  three r183 reads `quatf` real-last from text — cost if wrong: when three fixes its loader,
  animated text-USD views go wrong until this is removed (three is pinned at 0.183.2).
- The StrictMode fix shipped without an automated test (it is a dev-only double mount) —
  cost if wrong: the dev page shows the error box again.
- The loading story asserts that the load finishes, not the brief progress bar — cost if
  wrong: the bar's look is checked by eye only.
- Parity sends each sample's main file alone, as the HTTP API takes one file, and ran on
  litegen's debug binary during a quiet `mesh/` — cost if wrong: multi-file reads are compared
  only through their main file.

## Leftovers

- three.js's own loaders, which the tool names but does not work around:
  - the USD loader draws per-face materials (GeomSubset bindings) in one colour, and some
    quads in `displayColor`;
  - the USDZ loader skips a referenced prim, so Apple's sample draws 3 of its 4 meshes.
- The wasm package's FBX output differs from the native converter's by one ulp in a few
  doubles. Making them bit-identical would mean changing `mesh/`'s maths.
- Not set up: publishing (npm, PyPI, CI wheels for each OS), a free-threaded wheel (abi3 has no
  free-threaded form), and pickling of results.
- The app's zip preview normalises only separators. A zip whose names use percent escapes
  previews without those files, though it still converts.
- Each written model except a zip is read back once to measure its output stats.

## How to check it again

From `packages/3d-converter`:

```sh
cargo test -p litegen-3d-converter
node js/scripts/build.mjs && (cd js && npm test)
python/scripts/build.sh && (cd python && .venv314/bin/python -m pytest -q tests)
```

From `apps/3d-converter`, after `npm run build && npm run build-storybook`:

```sh
npx vitest run
PW_PROJECT=stories npx playwright test --project stories
PW_PROJECT=e2e npx playwright test --project e2e
```

For parity, run this with a litegen binary newer than `mesh/`:

```sh
LITEGEN_BINARY=litegen-core/target/debug/litegen node packages/3d-converter/scripts/parity.mjs
```
