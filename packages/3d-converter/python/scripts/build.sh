#!/usr/bin/env bash
# Builds the wheel for litegen-3d-converter into python/dist/:
#   1. regenerates litegen_3d_converter.pyi from the Rust source (stub_gen),
#      so the stub the wheel ships is current by construction;
#   2. builds the abi3 wheel with maturin (pinned), fat LTO, symbols stripped.
# Cargo is rustup's (rust-toolchain.toml); see packages/3d-converter/README.md
# for the shared-tree build rules.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$HOME/.local/bin:$PATH"
export CARGO_INCREMENTAL=0
cd "$here/.."
PYO3_PYTHON="${PYO3_PYTHON:-$(command -v python3)}" cargo run -q -p litegen-3d-converter-py --bin stub_gen --features stubgen
cd "$here"
uvx --from maturin==1.16.0 maturin build --release --out dist "$@"
