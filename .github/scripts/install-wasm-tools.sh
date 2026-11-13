#!/usr/bin/env bash
# install-wasm-tools.sh: the two tools packages/3d-converter/js/scripts/build.mjs
# runs, at the versions the converter's README names, on x86_64 Linux:
#   - wasm-bindgen 0.2.130 (it must equal the crate's version in the
#     converter's Cargo.lock; build.mjs checks) into packages/3d-converter/.tools/bin;
#   - wasm-opt from binaryen version_133, onto PATH through $GITHUB_PATH.
# Each download is checked against the sha256 its GitHub release lists.
set -euo pipefail
[[ $(uname -s)-$(uname -m) == Linux-x86_64 ]] || { echo "install-wasm-tools.sh: x86_64 Linux only"; exit 1; }
root=$(git rev-parse --show-toplevel)
tmp=${RUNNER_TEMP:-$(mktemp -d)}

fetch() { # <url> <sha256> <file>
  curl -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c -
}

bindgen=0.2.130
fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$bindgen/wasm-bindgen-$bindgen-x86_64-unknown-linux-musl.tar.gz" \
  9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f "$tmp/wasm-bindgen.tar.gz"
mkdir -p "$root/packages/3d-converter/.tools/bin"
tar -xzf "$tmp/wasm-bindgen.tar.gz" -C "$tmp"
install -m 755 "$tmp/wasm-bindgen-$bindgen-x86_64-unknown-linux-musl/wasm-bindgen" "$root/packages/3d-converter/.tools/bin/wasm-bindgen"

binaryen=version_133
fetch "https://github.com/WebAssembly/binaryen/releases/download/$binaryen/binaryen-$binaryen-x86_64-linux.tar.gz" \
  2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489 "$tmp/binaryen.tar.gz"
tar -xzf "$tmp/binaryen.tar.gz" -C "$tmp"
echo "$tmp/binaryen-$binaryen/bin" >> "${GITHUB_PATH:?set by GitHub Actions}"

"$root/packages/3d-converter/.tools/bin/wasm-bindgen" --version
"$tmp/binaryen-$binaryen/bin/wasm-opt" --version
