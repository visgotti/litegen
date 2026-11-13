# packages/3d-converter

litegen's 3D converter as two packages, built from one Rust source:

- **`@litegen/3d-converter`** on npm: WebAssembly, for Node and browsers
  ([js/README.md](js/README.md));
- **`litegen-3d-converter`** on PyPI: a native extension
  ([python/README.md](python/README.md)).

Both compile `litegen-core/src/mesh/` itself, the code litegen's HTTP
converter runs. Nothing is ported or forked, and no conversion rule lives in
JavaScript or Python. The browser tool in `apps/3d-converter` is built on the
npm package.

This page is for whoever builds, tests or updates the packages.

## Layout

| folder | what it is |
|---|---|
| `mesh/` | a `Cargo.toml` only. `[lib] path` points at `litegen-core/src/mesh/mod.rs`, so the folder compiles as the crate `litegen-mesh`, with nothing moved. `.claude/rules/mesh-standalone.md` keeps that folder liftable like this |
| `core/` | `litegen-3d-converter`: the typed, high-level API both bindings wrap. It runs the hosted converter's pipeline (`convert_bytes`) and its option rules (`parse_query`) |
| `js/` | the wasm-bindgen binding, its two entry files and the npm package |
| `python/` | the PyO3 binding, its stub generator and the wheel |
| `scripts/` | `parity.mjs`, the cross-check against litegen's own HTTP converter |
| `testdata/` | fixtures the three test suites share |

The workspace has its own `Cargo.lock`, seeded from `litegen-core/Cargo.lock`,
and its own `target/`.

## Tools

- **Rust**: rustup's toolchain, pinned by `rust-toolchain.toml` (1.99.1, the
  repo root's version, with the `wasm32-unknown-unknown` target). Homebrew's
  own Rust ignores the pin and ships no wasm32 standard library. On a Mac,
  Homebrew's rustup is keg-only, so its cargo is
  `/opt/homebrew/opt/rustup/bin/cargo`. The build scripts put it first on
  `PATH`.
- **wasm-bindgen-cli 0.2.130**, which must equal the crate's version, in a
  git-ignored folder:
  `cargo install wasm-bindgen-cli --version 0.2.130 --locked --root .tools`.
- **wasm-opt** from binaryen (`brew install binaryen`).
- **maturin 1.16.0**, run through `uvx` (pinned in `python/scripts/build.sh`).
- Node 22 or later, and Python 3.10 and 3.14 virtualenvs for the tests
  (`python/.venv310`, `python/.venv314`), with the pinned test tools of
  `python/requirements-test.txt` (`pytest`, `mypy`, `pyright`), which CI
  installs too:
  `uv pip install --python python/.venv314/bin/python -r python/requirements-test.txt`.

## Build

```sh
node js/scripts/build.mjs        # js/dist: cargo (release-wasm), wasm-bindgen --target web, wasm-opt -O3
python/scripts/build.sh          # the stub (stub_gen), then the abi3 wheel in python/dist
```

Both builds set `CARGO_INCREMENTAL=0`. In litegen's shared checkout, ask the
other sessions before running cargo, and never run it alongside theirs
(`.claude/rules/working-in-this-repo.md`).

## Test

```sh
cargo test -p litegen-3d-converter                                       # core
cargo clippy --all-targets -- -D warnings                                # every crate
cargo clippy -p litegen-3d-converter-js --target wasm32-unknown-unknown -- -D warnings
(cd js && npm test)                                                      # node:test + tsc on the built dist
(cd python && .venv314/bin/python -m pytest -q tests && .venv310/bin/python -m pytest -q tests)
```

The JS and Python suites test the built packages: build first.

- **core**: detection, reading, writing, every format pair, the option rules,
  and the change detectors below.
- **js**: the package as Node imports it. That covers every function and
  error, memory (an over-cap input never enters WebAssembly memory), the
  README's examples, a doc comment on every declaration in the `.d.ts`,
  `npm pack`'s file list, the size budget, and a `tsc` check of typed usage,
  including lines that must not compile.
- **python**: the same API checks, the GIL released, threads sharing a model,
  bytes borrowed without a copy, the README's doctests, mypy's `stubtest`,
  mypy `--strict` and pyright strict over a typed usage file, a docstring on
  every stub declaration, and a check that the stub is current.

## When litegen's converter changes

The core copies three items from outside `mesh/`: `convert_bytes`'s pipeline,
`parse_query`'s option rules, and the media-type table
(`store/mod.rs`). No compiler sees those change, so detectors watch them:

- **`core/tests/handler_tripwire.rs`** hashes the copied items of
  `litegen-core/src/api/handlers/convert.rs` and `store/mod.rs`'s
  `model3d_content_type`. When it fails, re-read what
  changed there, carry the change into `core/src/model.rs` and
  `core/src/options.rs` with a test, then update `PINNED`.
- **`core/tests/mesh_crate.rs`** fails when `mesh/Cargo.toml`'s dependency
  specs or their locked versions differ from litegen-core's.
- **`every_humanoid_table_and_bone_of_mesh_is_typed`** (core/tests/read.rs)
  reads `mesh/humanoid.rs`. A naming scheme or bone that mesh adds must be
  added to `HumanoidNaming` or `VrmBone` in `core/src/info.rs`, then to the
  JS mirror in `js/src/api.rs`.
- Every conversion from a mesh enum, and from a core enum in either binding,
  is an exhaustive `match`, so a new value fails the build at the line to
  extend.

After a change, rebuild both packages, which also regenerates the Python
stub, and run all three suites.

## How the wasm build was chosen

`js/scripts/bench.mjs` runs three real models into all ten formats, plus a
520,000-triangle mesh into seven, and reports the median of five runs.
Eight variants, all passing the tests:

| opt-level | SIMD | wasm-opt | raw KiB | gzip KiB | brotli KiB | median ms |
|---|---|---|---|---|---|---|
| **3** | **off** | **-O3** | **3237** | **1193** | **882** | **826** |
| 3 | off | -Oz | 3221 | 1193 | 883 | 835 |
| 3 | on | -O3 | 3264 | 1206 | 891 | 849 |
| 3 | on | -Oz | 3243 | 1206 | 892 | 837 |
| s | off | -O3 | 2352 | 935 | 731 | 961 |
| s | off | -Oz | 2335 | 936 | 733 | 963 |
| s | on | -O3 | 2331 | 927 | 728 | 970 |
| s | on | -Oz | 2315 | 929 | 729 | 938 |

The rule was: the fastest, unless a variant within 5% of its speed is 15%
smaller. opt-level `s` is 17% smaller but 13–15% slower, and simd128 bought
no speed. So the build is opt-level 3, fat LTO, one codegen unit, `panic =
"abort"` and `wasm-opt -O3`, without simd128. `js/test/pack.test.mjs` fails
when the brotli size grows more than 10% past its last baseline: 1041 KiB since mesh/ gained Draco and meshopt decoding (882 KiB when the variants were measured).

The Python wheel is built with opt-level 3, fat LTO, one codegen unit and
stripped symbols. Panics unwind, so a bug surfaces as Python's
`PanicException` instead of a crashed interpreter.

## Parity with the hosted converter

```sh
node scripts/parity.mjs
```

This converts every sample in `apps/3d-converter/public/samples` to every
target, as one file and as a zip, three ways: litegen's HTTP converter (its
release binary on a free port), the npm package and the Python package. It
compares the bytes and prints the pairs that differ. It never builds litegen.
It runs only when `litegen-core/target/release/litegen` is newer than
`mesh/` and `convert.rs`, and refuses otherwise. The HTTP converter takes one
file, so every sample is sent as its main file alone.

The Python package and litegen's binary share a platform's libm, so they
match byte for byte. The wasm package brings its own (Rust's bundled port),
so a float that goes through a transcendental function can differ in its last
bit: FBX's Euler angles, quaternion slerp and the sRGB curves use one. Across
native platforms the same holds: Apple's libm on macOS, glibc's on Linux.
Making every platform bit-identical would mean changing `mesh/`'s maths.

The run of 2026-11-05 (litegen's debug binary, which writes the same bytes as
a release one) compared 180 conversions. The Python package matched all 180.
The npm package matched 174; the other six were its FBX writes, which differ
in one to four doubles by one unit in the last place. The 20 conversions of
the multi-file glTF sample can't be compared, because its main file alone
points at a `.bin` the HTTP request doesn't carry.

## Publishing

Not set up here. For npm: `npm publish` from `js/` after a build (the package
holds `dist/` only). For PyPI: build a wheel on each platform with
`python/scripts/build.sh` (or `maturin build --release` in CI, one abi3 wheel
per OS and architecture), then upload with `twine` or `maturin upload`.
