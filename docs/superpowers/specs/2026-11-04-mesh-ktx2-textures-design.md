# KTX2 (Basis Universal) textures to PNG — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: L.
Research: [`research-codecs.md`](../research/research-codecs.md) §3. It's
git-ignored, so its key facts are copied here.

## Why

The scene carries KTX2 images as bytes (`ImageFormat::Ktx2`), but nothing in-process
decodes Basis Universal. On every production path, delivery and the converter, a KTX2
texture is **dropped from every output, GLB included**. `normalize::transcode_textures`
removes it from its materials, the delivery note says "texture {t} could not be decoded
and was left out", and the converter adds an `image_formats` loss. Only the library's
own `write_scene` keeps it, in glTF. A vendor or caller GLB using `KHR_texture_basisu`
(gltfpack `-tc`/`-tu`, glTF-Transform's `etc1s`/`uastc`, and many web-optimised
assets) loses its textures everywhere litegen writes it.

## What the research established

- **KHR_texture_basisu allows two payloads:**
  - ETC1S with BasisLZ (supercompression 1);
  - UASTC with no supercompression or with Zstandard (0 or 2).

  ZLIB isn't allowed. Mipmaps are a SHOULD. Dimensions MUST be multiples of 4, and
  material maps 2D.
- **Decode `levels[0]`,** the largest level. Its bytes are found by `byteOffset` and
  `byteLength`, because level data runs smallest first in the file. `levelCount = 0`
  means base level only.
- **Transcoding to RGBA does no colour conversion.** The output is the stored values,
  which is what PNG and the glTF slots expect.
- **The samples' DFD colour tags are often wrong** (ORM maps tagged sRGB, for
  example), so the DFD isn't used to convert anything.
- **Every UASTC sample is Zstandard-compressed.** Zstandard is the one piece with no
  std-only option in the tree: `miniz_oxide` covers ZLIB only.
- **Sizes:**
  - ETC1S → RGBA is about 1,100–1,500 C++ code lines in Binomial's transcoder;
  - UASTC → RGBA is about 1,000;
  - the KTX2 container and BasisLZ global data are about 1,000;
  - a std-only Rust total is estimated at 3–4.5 k lines, plus 1.5–2.5 k for a
    hand-written Zstandard decoder.
- **Crates:**
  - `ruzstd` 0.9.1 (MIT): pure-Rust Zstandard, mature (77 M downloads; used by
    bevy_image, `object` and `backtrace`), fuzzed, no runtime deps with
    `default-features = false, features = ["std"]`. Its ring buffer uses 38 lines of
    `unsafe`;
  - `ktx2` 0.5 (Apache-2.0): a pure-Rust container parser, which saves about 500 lines;
  - the pure-Rust transcoders (`basisu` 0.1.0, the unpublished `draco-texture`) are
    days or weeks old;
  - the mature ones wrap C++.
  - Bevy itself can't decode ETC1S in KTX2, and uses C++ for UASTC.

## Decisions

### What gets built (mine)

- A new std-only file, `mesh/ktx2.rs`:
  - parses the KTX2 container: identifier, header, level index, DFD, and the BasisLZ
    supercompression global data;
  - decodes **level 0, layer 0, face 0** of:
    - **ETC1S/BasisLZ** (KTX2 Appendix D: Huffman tables, endpoint and selector
      codebooks, slice decoding, with an RGB slice plus an optional alpha slice) to
      ETC1 blocks, then RGBA8;
    - **UASTC LDR 4×4** blocks to RGBA8;
    - **uncompressed `VK_FORMAT_R8G8B8A8_UNORM`/`_SRGB`**, copied, because it's
      trivial and appears in non-Basis KTX2 files.

  Anything else (BCn, ASTC, HDR UASTC, cube maps, arrays, 3D, ZLIB supercompression) is
  `Unsupported`, named.
- `normalize::transcode_textures` decodes KTX2 through `ktx2::decode_rgba8` and encodes
  PNG with `image`, which normalize.rs may use, exactly as it does for WebP. It writes
  the bytes as stored, with no sRGB conversion from the DFD. A KTX2 that fails to decode
  is still dropped and reported as today, with the decoder's reason added to the note.
- **glTF outputs get PNG too,** consistent with the WebP policy: a delivered file never
  requires an extension. The vendor's original is delivered beside it as `original`,
  KTX2 intact.

### Zstandard (D3, settled with the user: `ruzstd`, fully tested)

The user: "do whatever is most stable … make sure it's fully tested completely". `ruzstd`
is the stable choice:
- MIT;
- 21 releases since 2019, 77 M downloads, 308 reverse dependencies (bevy_image,
  `object`, `backtrace`);
- fuzzed upstream;
- no runtime dependencies with `default-features = false, features = ["std"]`.

A hand-written decoder would be new, untested code on the same hostile path. So:
- **`ruzstd = { version = "=0.9.1", default-features = false, features = ["std"] }`,**
  pinned exactly and confined to `mesh/ktx2.rs`. `isolation_tests.rs`' `ALLOWED` gains it
  for that file only, as the standalone rule requires for a user-approved crate.
- **"Fully tested", for a decoder we didn't write:**
  - **Bit-exact against the reference `zstd`.** A corpus made once with the `zstd` CLI
    (1.5.x; version recorded) from our own data: UASTC level data of our fixtures,
    repeated-byte buffers (RLE blocks), random bytes (raw blocks), and structured data
    (compressed blocks with FSE and Huffman literals, repeat offsets). It's encoded at
    levels 1, 3, 9, 19 and 22 (`--ultra`), with and without `--check`, with
    `--long=27`, as several concatenated frames, and with a skippable frame first.
    Inputs and `zstd -d` outputs are committed, as our content.
    `ruzstd_matches_reference_zstd` requires byte-equal output for every file;
  - **Hostile input:**
    - a frame whose declared content size passes the expected level length is refused
      at the cap, so the decoder is handed that exact length as its output limit;
    - a frame naming a dictionary id is refused, because KTX2 uses none;
    - truncation and bit flips over every committed frame: no panic, and every error
      maps to `MeshError::Malformed`;
  - **Miri over the `unsafe`:** the Zstandard tests run once under `cargo +nightly miri
    test` on their small inputs. That exercises `ruzstd`'s raw-pointer ring buffer
    under undefined-behaviour checks. The result is recorded, and any Miri error is a
    blocker reported upstream;
  - **Supply chain:** `cargo audit` reports no advisory for `ruzstd =0.9.1`, and
    `cargo tree` shows it with no runtime dependencies. Both are recorded, and the
    whole set reruns whenever the pin changes.

### An option for callers: `ktx2_textures` (opt-in; see the overview's options table)

- `png` (the default) is everything above: KTX2 becomes PNG for every container.
- `keep` keeps the KTX2 in **glb and gltf** outputs. The material references it through
  `KHR_texture_basisu`, which then goes in `extensionsRequired`: the caller chose GPU
  textures over portability. The KTX2 bytes are byte-identical to the source's, and they
  don't count against the PNG texture budget. Every other container in the same request
  still gets PNG.
- It's offered as a converter query parameter and as a 3D request field, recorded in the
  `DeliveryPlan`. `keep` with `keep_outputs: false` is 400 `option_needs_keeping`.

## Non-goals

- Encoding KTX2. `ktx2_textures=keep` only keeps a source's KTX2 as it came.
- HDR UASTC, ASTC, BCn and other block formats, cube maps and texture arrays.
- Mip levels below 0.
- Colour-space conversion from DFD tags.

## Hostile input and limits

- **Pixels:** width × height ≤ 64M, the existing `normalize::decode_image` budget of
  256 MiB of RGBA. This is checked from the header before anything is allocated.
- **A scene-wide output budget** (builder check). A PNG is often several times the
  ETC1S or UASTC + Zstd it replaces, so a scene near the 128 MiB input cap could
  transcode into a GLB that litegen's own reader refuses. Then delivery's `manifest()`
  is `None`, and memory grows past the measured 5–13×.
  - `transcode_textures` transcodes one image at a time, in image order, and keeps a
    running total of the scene's encoded image bytes, PNG and JPEG alike.
  - It stops before the total would pass **`MAX_INPUT_BYTES / 2` (64 MiB)**, which
    leaves the other half for geometry so the written file stays readable.
  - Each KTX2 image past the budget is dropped as today, with the note "texture {t}
    (KTX2) was left out: transcoding it would pass the 64 MiB texture budget".
  - *(Built, Ruling K18: the total counts the images already PNG or JPEG first, then
    each WebP's PNG, then each KTX2 in image order, stopping at the first KTX2 that
    would pass the budget. Images that need no decode are never dropped, so they count
    first, and "in image order" holds among the KTX2.)*
  - The policy stays PNG everywhere. A glTF that kept KTX2 past the budget would need
    `KHR_texture_basisu` in `extensionsRequired`, which delivery never ships.
    *(A caller's opt-in `ktx2_textures=keep` is the one exception: a glb or gltf it
    keeps a KTX2 in requires `KHR_texture_basisu`, and that KTX2 isn't charged to the
    budget.)*
- **Structure:**
  - level 0's `byteOffset + byteLength` lies inside the file;
  - the DFD and SGD offsets and lengths lie inside the file;
  - BasisLZ `imageDescs` slice offsets lie inside level 0;
  - Huffman tables are well-formed (code lengths sum within the Kraft limit), and
    codebook sizes are bounded by the header's declared counts, checked against the
    bytes present.
- **Zstandard:** the inflated length must equal ceil(w/4) × ceil(h/4) × 16 exactly. The
  decoder is given that as its output cap, so a bomb stops at the cap.
- Truncating or flipping any byte never panics (the overview's sweeps).

## What proves it is done

### Bit-exact against the reference transcoder (the main proof)

- **Inputs, committed in `testdata/ktx2/`** with a README of sources and licences:
  - CC0 FlightHelmet `LensesMat_Normal.ktx2` (1,685 B, UASTC + Zstd) and
    `LensesMat_BaseColor.ktx2` (93 KB, ETC1S);
  - CC-BY CarConcept `Rib_N.ktx2` (622 B, UASTC) and `Dot_C.ktx2` (1.4 KB, ETC1S), with
    attribution;
  - our own textures from `fixtures::quadrant_png()` and `bands_png()`, encoded by
    Binomial's `basisu` v2_50 (or KTX-Software `toktx`) as ETC1S, ETC1S with alpha,
    UASTC, and UASTC + Zstd. These are our content.
- **Expected outputs:** each input transcoded once to raw RGBA8 by the reference
  (Binomial `basisu -unpack`, or KTX-Software `ktx extract --transcode rgba8`), with the
  tool and version in the README. The outputs are committed as raw `.rgba` files.
  *(Built: Zstandard-compressed `.rgba.zst` files, which the test inflates with `ruzstd`
  before the byte-for-byte compare, so they stay bit-exact at a fraction of the size
  (Ruling K10). The reference transcoder is Binomial's own, as bundled with three.js r183
  (`basis_transcoder.{js,wasm}`) and run under Node, because building `basisu` waits on
  the user (Rulings K6 and K9). `testdata/ktx2/README.md` names the tool and version.)*
- `every_reference_texture_transcodes_bit_exactly`: `ktx2::decode_rgba8(input) ==
  expected`, byte for byte, for every file. ETC1S and UASTC to RGBA are deterministic,
  so any difference is a decoder bug, not a tolerance question.

### Unit tests (ktx2.rs), RED first

- **The container:**
  - `levels_are_taken_by_offset_not_by_order` (level data smallest-first);
  - `level_count_zero_means_the_base_level`;
  - `zlib_supercompression_is_refused_by_name`;
  - `cube_and_array_textures_are_refused_by_name`.
- **Limits:**
  - `a_texture_past_the_pixel_budget_is_refused_before_allocating` (a header of 65,536
    × 65,536);
  - `a_zstd_payload_longer_than_its_level_is_malformed`;
  - `slice_offsets_outside_level_zero_are_malformed`;
  - `a_malformed_huffman_table_is_malformed`.
- `an_etc1s_alpha_slice_becomes_alpha`: the RGB + AAA case equals the reference's RGBA.
- `the_texture_budget_drops_the_rest_and_says_so` (normalize.rs): a scene of KTX2
  textures whose PNGs together pass 64 MiB. The early ones transcode, the later ones
  are dropped with the budget note, and the scene written as GLB reads back with
  `read_scene`.
- `dfd_tags_never_change_the_bytes`: the same payload with the DFD tag flipped between
  sRGB and linear decodes to identical bytes.
- The truncation and bit-flip sweeps on all six committed inputs.

### Integration

- `ktx2_textures_survive_every_container` (tests.rs): a GLB using `KHR_texture_basisu`
  (gltfpack `-tc` and `-tu` on the `textured` fixture, committed as our content),
  through `transcode_textures`, then written to every container that holds textures.
  Each keeps its base-colour texture as PNG, no `image_formats` loss is reported, and
  the PNG's pixels equal the reference RGBA.
- **Converter API:** `to=fbx` on that GLB gives a `X-Litegen-Conversion-Losses` header
  without `image_formats`. A broken KTX2 still gives the loss.
- **Delivery** (model3d_api): a mock row returning the `-tc` GLB delivers every promised
  format with textures. The delivered GLB's images are PNG, and the `original` asset is
  byte-identical to the vendor's file.

### Render harness

- `--import` (local) runs the Khronos KTX-BasisU variants: FlightHelmet,
  ChronographWatch, StainedGlassLamp, AnisotropyBarnLamp and CarConcept. The native
  check renders the original with three.js `GLTFLoader` and `KTX2Loader`, whose
  transcoder WASM is in three r183's `examples/jsm/libs/basis/`, against our converted
  output.
  - The browser transcodes to whatever GPU format it supports, which isn't always
    RGBA32, so the albedo threshold comes from a calibration run, recorded in the
    harness README.
  - The bit-exact unit test above is the strict proof, and the render is the
    integration proof.
- Negative control: swap the ETC1 selector bit order in the decoder.
  `every_reference_texture_transcodes_bit_exactly` fails on every ETC1S input, and the
  native albedo cases fail. Restore by rewriting.

### Docs

- `normalize.rs`'s and `scene.rs`'s `ImageFormat` docs ("nothing in-process decodes"
  goes);
- `3d-format-conversion.md` §3 and §9;
- the converter's OpenAPI description (KTX2 joins WebP as re-encoded to PNG);
- the matrix page's "Textures" cell notes (via litegen-a3).

### The `ktx2_textures` option

- `ktx2_textures_keep_leaves_glb_its_ktx2`: on the `-tc` GLB, `to=glb&ktx2_textures=keep`
  keeps the image bytes identical to the source, with `KHR_texture_basisu` in
  `extensionsRequired`. The Khronos validator reports 0 errors, and three.js
  `GLTFLoader` with `KTX2Loader` renders it.
- `ktx2_textures_keep_still_gives_other_containers_png`: a request with `[glb, fbx,
  usdz]` and `keep` gives KTX2 in the GLB and PNG in the FBX and USDZ.
- `ktx2_textures_png_is_todays_output`: the default is byte-identical to the
  no-parameter output.
- The validation tests: a bad value gives 400 `param_invalid`. `keep` with keep-nothing
  gives 400 `option_needs_keeping`. The poller path honours the recorded plan.

### Gate

The overview's full gate, the harness `--import` set, and the D3 checks above (the
reference-`zstd` corpus, Miri, `cargo audit`).

## Left to the builder

- Whether to use the `ktx2` crate for the container. That's another crate, and the user's
  call. The default is to parse it std-only: about 500 lines, and the format is small.
- The internal structure of the ETC1S and UASTC decoders. Port from KTX2 Appendix D and
  Binomial's transcoder at `073eb6a0a34a`.
