#!/bin/sh
# make_reference.sh: regenerate the reference corpus that
# draco_core_matches_googles_decoder (litegen-core/src/mesh/draco.rs) reads,
# streams/ and manifest.txt, from google/draco's own encoder and decoder.
# A one-off that records every command (mesh-gaps item 2, Task 4b); README.md
# beside it says what each step makes.
#
#   WORK=<scratch folder outside the repo> sh make_reference.sh [step ...]
#
# With no step named, it runs them all, in this order:
#   tools     clone google/draco at DRACO_COMMIT, check the commit, build
#             libdraco, draco_encoder and draco_decoder with CMake, and compile
#             encode_reference.cc and dump_reference.cc against them;
#   fixtures  litegen's fixtures as its own GLB writer writes them, through the
#             ignored test glb.rs keeps for that (cargo, in this checkout);
#   geometry  every triangle primitive of the inputs as a geometry file, and
#             inputs.tsv, the inputs' table, beside this script;
#   streams   every input at every setting, into $WORK/streams-all, then each
#             byte-distinct stream once into streams/ (replacing it), under
#             the first (input, setting) pair that wrote it (Ruling B42);
#   dumps     Google's decoding of every stored stream, raw, and
#             draco_decoder's own PLY of it;
#   manifest  manifest.txt and streams.inc from the dumps and the pairs, then
#             the seqc/cl0 equivalence check and the PLY cross-check.
#
# Optional: DRACO_SRC, DRACO_BUILD (default $WORK/draco-src, $WORK/draco-build),
# BIN (default $WORK/bin), JOBS (CMake's parallel jobs, default 4).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
TESTDATA=$(cd "$HERE/../.." && pwd)        # litegen-core/src/mesh/testdata/gltf
CORE=$(cd "$TESTDATA/../../../.." && pwd)  # litegen-core
WORK=${WORK:?set WORK to a scratch folder outside the repo}
DRACO_SRC=${DRACO_SRC:-$WORK/draco-src}
DRACO_BUILD=${DRACO_BUILD:-$WORK/draco-build}
BIN=${BIN:-$WORK/bin}
JOBS=${JOBS:-4}

DRACO_REPO=https://github.com/google/draco.git
# 1.5.9 and one commit ("Pin three.js example to r125 (#1252)"), which changes
# javascript/example/webgl_loader_draco.html alone.
DRACO_COMMIT=d429de84b52a860705507da0884764c5d8321423

FIXTURES="cube layered metal morph multiuv multiuv_shared skinned staged textured"
SAMPLES="Box MorphPrimitivesTest RiggedSimple Avocado WaterBottle"
LEVELS="0 1 5 7 10"
POSITION_BITS="8 11 14 16"
EXTRAS="seqc-qp11 swap-qp11 meta-qp11"

tools() {
  if [ ! -d "$DRACO_SRC" ]; then
    git clone --revision="$DRACO_COMMIT" --depth 1 "$DRACO_REPO" "$DRACO_SRC"
  fi
  test "$(git -C "$DRACO_SRC" rev-parse HEAD)" = "$DRACO_COMMIT"
  test -z "$(git -C "$DRACO_SRC" status --porcelain)"
  cmake --version | head -1
  c++ --version | head -1
  # -ffp-contract=off: strict IEEE evaluation of draco's own source, what its
  # x86-64 baseline and WebAssembly builds compute. Clang's default on arm64
  # fuses `x * x + y * y + z * z` in the octahedral normal transform into
  # fused multiply-adds, which moves 414 streams' normals by up to 2.7e-7
  # (README.md, Ruling B45).
  cmake -S "$DRACO_SRC" -B "$DRACO_BUILD" -G Ninja -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_EXPORT_COMPILE_COMMANDS=ON \
    -DCMAKE_C_FLAGS=-ffp-contract=off -DCMAKE_CXX_FLAGS=-ffp-contract=off
  cmake --build "$DRACO_BUILD" --target draco_static draco_encoder draco_decoder -j "$JOBS"
  mkdir -p "$BIN"
  cp "$DRACO_BUILD/draco_encoder" "$DRACO_BUILD/draco_decoder" "$BIN/"
  # The flags CMake gives draco's own tools (compile_commands.json), with the
  # library's headers as system headers so that -Wall -Wextra speak of our
  # code alone: its sources, and draco/draco_features.h, which CMake writes
  # into the build folder.
  for tool in encode_reference dump_reference; do
    c++ -std=c++17 -O3 -DNDEBUG -DDRACO_CMAKE=1 -D_FILE_OFFSET_BITS=64 -D_LARGEFILE_SOURCE \
      -ffp-contract=off -Wall -Wextra -isystem "$DRACO_SRC/src" -isystem "$DRACO_BUILD" \
      "$HERE/$tool.cc" "$DRACO_BUILD/libdraco.a" -o "$BIN/$tool"
  done
}

fixtures() {
  mkdir -p "$WORK/inputs/fixtures"
  # Several sessions share this checkout's target/: wait out any build.
  while pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null || pgrep -x clippy-driver >/dev/null; do
    sleep 45
  done
  (cd "$CORE" && GLTFPACK_INPUTS_OUT="$WORK/inputs/fixtures" CARGO_INCREMENTAL=0 \
    cargo test --lib -- --ignored mesh::glb::tests::write_the_gltfpack_inputs)
}

geometry() {
  set --
  for f in $FIXTURES; do
    set -- "$@" "fixture-$f=$WORK/inputs/fixtures/$f.glb"
  done
  for s in $SAMPLES; do
    set -- "$@" "sample-$s=$TESTDATA/draco/$s/glTF/$s.gltf"
  done
  set -- "$@" "quantized-textured=$TESTDATA/draco/ours/textured.q.glb"
  set -- "$@" "gltfpack-skinned=$TESTDATA/gltfpack/skinned.glb"
  rm -rf "$WORK/geometry"
  python3 -I "$HERE/reference.py" geometry "$WORK/geometry" "$@"
  cp "$WORK/geometry/inputs.tsv" "$HERE/inputs.tsv"
}

streams() {
  rm -rf "$WORK/streams-all" "$HERE/streams"
  mkdir -p "$WORK/streams-all"
  for g in "$WORK/geometry"/*.geom; do
    input=$(basename "$g" .geom)
    for c in $LEVELS; do
      for q in $POSITION_BITS; do
        "$BIN/encode_reference" "$g" "$WORK/streams-all/$input.cl$c-qp$q.drc" "cl$c-qp$q"
      done
    done
    for setting in $EXTRAS; do
      "$BIN/encode_reference" "$g" "$WORK/streams-all/$input.$setting.drc" "$setting"
    done
  done
  python3 -I "$HERE/reference.py" store "$WORK/streams-all" "$HERE/streams" "$WORK/pairs.tsv"
}

dumps() {
  rm -rf "$WORK/dumps" "$WORK/ply"
  mkdir -p "$WORK/dumps" "$WORK/ply"
  for s in "$HERE/streams"/*.drc; do
    stream=$(basename "$s" .drc)
    mkdir "$WORK/dumps/$stream"
    # A stream Google's decoder refuses is recorded, not skipped: its
    # refused.txt holds the message, and the manifest says so.
    if ! "$BIN/dump_reference" "$s" "$WORK/dumps/$stream" > "$WORK/dumps/$stream/description.txt" \
        2> "$WORK/dumps/$stream/refused.txt"; then
      echo "Google's decoder refused $stream: $(cat "$WORK/dumps/$stream/refused.txt")"
    else
      rm "$WORK/dumps/$stream/refused.txt"
    fi
    # draco_decoder's PLY writer names float, uchar and int alone, so for
    # other types its file is unreadable; the cross-check skips those.
    "$BIN/draco_decoder" -i "$s" -o "$WORK/ply/$stream.ply" > /dev/null 2>&1 ||
      echo "draco_decoder wrote no PLY for $stream"
  done
}

manifest() {
  python3 -I "$HERE/reference.py" manifest "$HERE/streams" "$WORK/dumps" "$WORK/geometry" "$WORK/pairs.tsv" \
    "$HERE/manifest.txt" "$HERE/streams.inc"
  python3 -I "$HERE/reference.py" equivalence "$WORK/dumps" "$WORK/pairs.tsv"
  python3 -I "$HERE/reference.py" crosscheck "$WORK/dumps" "$WORK/ply"
}

[ $# -gt 0 ] || set -- tools fixtures geometry streams dumps manifest
for step in "$@"; do
  case $step in
    tools | fixtures | geometry | streams | dumps | manifest) "$step" ;;
    *) echo "unknown step: $step" >&2; exit 2 ;;
  esac
done
