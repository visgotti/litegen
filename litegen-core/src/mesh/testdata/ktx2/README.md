# KTX2 (Basis Universal) test vectors

Real KTX2 textures and the raw RGBA8 each one must decode to, the latter kept
Zstandard-compressed (`<name>.rgba.zst`, below). They are the reference corpus
of mesh-gaps item 3 (`ktx2.rs`): `the_corpus_parses` and the truncation and
bit-flip sweeps read the inputs, and
`every_reference_texture_transcodes_bit_exactly` decodes each `.ktx2` and
compares the result with its reference, inflated by `ruzstd`, byte for byte.

## The inputs

Five downloaded files, **not our content**. Each is byte-identical to upstream
(sha256 below), from Khronos's glTF-Sample-Assets at commit
`8f492390ddc7a2ae221a1369ef560589cf8c70fb` (main on 2026-10-23, fetched
2026-11-08). The URL of each is
`https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/8f492390ddc7a2ae221a1369ef560589cf8c70fb/<path>`.

| file | path in the repository | bytes | sha256 | licence |
|---|---|---:|---|---|
| `FlightHelmet_Materials_LensesMat_Normal.ktx2` | `Models/FlightHelmet/glTF-KTX-BasisU/` | 1,685 | `b240ec01ac1507a1310d765a277055d731ba9cd432cc905715cfba026f6e7fb8` | CC0 1.0 |
| `FlightHelmet_Materials_LensesMat_BaseColor.ktx2` | `Models/FlightHelmet/glTF-KTX-BasisU/` | 93,420 | `f9f709a21f50974ec7fb3f3098f0968954822e1cde0d35a375bc23acbcd8e5f7` | CC0 1.0 |
| `Rib_N.ktx2` | `Models/CarConcept/glTF-KTX-BasisU-Draco/` | 622 | `7bbd1d7776a087b48d3f7d50395d24840fd00dc5ab2622f8dce5685995df94d3` | CC BY 4.0 |
| `Dot_C.ktx2` | `Models/CarConcept/glTF-KTX-BasisU-Draco/` | 1,374 | `08968e9d7d9855ae8d201cfaea889e2748fc9049a95838aa18ce689d29c041ca` | CC BY 4.0 |
| `StainedGlassLamp_grill_basecolor-alpha.ktx2` | `Models/StainedGlassLamp/glTF-KTX-BasisU/` | 217,120 | `1ad2e5f610b99ea673ea332818d4a43413808253966266436cbfba810c62dd42` | CC BY 4.0 |

What each is, from the transcoder's report and the header read straight from the
bytes (`make_reference.mjs` checks the two agree):

| file | format | scheme | size | levels | DFD transfer | alpha |
|---|---|---|---|---:|---|---|
| `FlightHelmet_Materials_LensesMat_Normal` | UASTC LDR 4x4 | Zstandard | 1024 x 1024 | 11 | linear | none |
| `FlightHelmet_Materials_LensesMat_BaseColor` | ETC1S | BasisLZ | 1024 x 1024 | 11 | sRGB | none |
| `Rib_N` | UASTC LDR 4x4 | Zstandard | 32 x 32 | 6 | linear | none |
| `Dot_C` | ETC1S | BasisLZ | 128 x 128 | 8 | sRGB | none |
| `StainedGlassLamp_grill_basecolor-alpha` | ETC1S | BasisLZ | 1024 x 1024 | 11 | sRGB | **an alpha slice** (`alphaSliceByteLength` past 0); 636,213 of 1,048,576 pixels are translucent |

All five have `vkFormat` 0 (a Basis payload), one face, no array layers and
power-of-two sizes that are multiples of four. **No input has a
non-power-of-two size.** Not one of the 60 `.ktx2` files in glTF-Sample-Assets
at this commit does (checked from each file's header); the case waits for an
encoder of ours (or a sample from outside glTF-Sample-Assets). The
ETC1S-with-alpha case is covered by the fifth file: none of the four files the
design names has an alpha slice, so it is the smallest ETC1S texture with one in
the sample set. Every file there up to 320 KB was checked with the transcoder's
`getHasAlpha`, and this is the only one that has alpha. The
other `-alpha` texture, `StainedGlassLamp_glass_basecolor-alpha`, is UASTC
(scheme 2) and 1.7 MB.

The names are upstream's, so the first two carry a `FlightHelmet_Materials_`
prefix that the design's short names (`LensesMat_Normal`, `LensesMat_BaseColor`)
leave out. The folder holds these files, the references, this README,
`make_reference.mjs` and `make_uastc_modes.mjs`, and a folder of its own,
`zstd/`, with the Zstandard corpus that tests `ruzstd` against the reference
`zstd` tool (its README says what is in it).
The tests load every file with `include_bytes!`, and `the_corpus_parses` embeds
this README too and fails unless the first column of the tables here (the input
files above and the references below) lists exactly the `.ktx2` and `.rgba.zst`
files the tests embed, each once.

### Licences and attribution

- **FlightHelmet** (the first two): Creative Commons Zero v1.0 Universal, as
  `Models/FlightHelmet/LICENSE.md` says. (c) 2018, Public. Gary Hsu for the
  conversion from Maya. The textures were compressed with glTF-Transform
  v4.3.0 (`etc1s` for the base colour, `uastc` for the normal), default settings.
- **CarConcept** (`Rib_N`, `Dot_C`): Creative Commons Attribution 4.0
  International (<https://creativecommons.org/licenses/by/4.0/legalcode>), as
  `Models/CarConcept/LICENSE.md` says. Attribution as its README gives it:
  "(c) 2024, Darmstadt Graphics Group GmbH. Eric Chadwick for Model and
  textures", from a public domain 3D model by Unity Fan. The textures are
  BasisU, ETC1S for the colour maps and UASTC for the normal maps.
- **StainedGlassLamp** (`StainedGlassLamp_grill_basecolor-alpha`): Creative
  Commons Attribution 4.0 International
  (<https://creativecommons.org/licenses/by/4.0/legalcode>), as
  `Models/StainedGlassLamp/LICENSE.md` says. Attribution as its README gives it:
  "(c) 2021, Wayfair. Eric Chadwick for Everything". Compressed with
  `toktx --genmipmap --bcmp --clevel 4 --qlevel 255`.

The CC BY 4.0 files are provided by their licensors as-is, with no warranties of any
kind; see the disclaimer in section 5 of the licence linked above.

The `.ktx2` files are unmodified. Each `.rgba.zst` is a decoded copy of its
input's pixels, so it carries the input's licence and attribution too. Nothing
of ours is mixed in.

## Our own input: `uastc_all_modes`

| file | bytes | sha256 | licence |
|---|---:|---|---|
| `uastc_all_modes.ktx2` | 4,000 | `3f3032ca9c32f6f326f09fcb5ba22b70bc960f7164eb8648bebe3e3ec2da4727` | ours; no third-party content |

The Khronos files above hold few UASTC blocks of few modes (a normal map is
mostly one mode), so they cannot show that every one of UASTC's 19 modes decodes
bit for bit. This file does. It is a 62 x 59 UASTC LDR 4x4 texture
(`vkFormat` 0, UASTC colour model, one level, **no supercompression**), whose
16 x 15 = 240 blocks are pseudo-random bits with the fields that pick a path set
on purpose (the script's header lists them):

- every mode but the reserved code 19: modes 0 to 18, mode 8 being the solid colour;
- every common partition pattern: all 30 two-subset patterns in each of modes 2, 4,
  9 and 16, all 11 three-subset ones in mode 3, all 19 of mode 7's own;
- every colour component selector (0 to 3) in the dual-plane modes 6, 11 and 13,
  and mode 17, whose selector is fixed;
- random ETC1/BC1 hint bits in every mode (a decoder must ignore them), random
  endpoints and weights, so every endpoint range and weight range is used;
- a width and height that are not multiples of 4, so the last column of blocks is
  2 pixels wide and the last row 3 high: the cut-block path.

It is generated by `make_uastc_modes.mjs` in this folder from a xorshift64 stream
(seed `0x9E3779B97F4A7C15`): the bits are our own, no image went in, and no encoder
made them. The reference transcoder (below) says what each block means, so
`uastc_all_modes.rgba.zst` is whatever Basis Universal v1.50 makes of the bits; the
script checks that the transcoder accepts the file, that two transcodes agree, and
that the plan reached every mode, pattern and selector. It is deterministic.
`the_all_modes_texture_reaches_every_mode_pattern_and_selector` checks the file
again from its bytes. To regenerate, from the repository root:

```sh
out=$(mktemp -d)
node litegen-core/src/mesh/testdata/ktx2/make_uastc_modes.mjs \
  scripts/mesh-verify/node_modules/three/examples/jsm/libs/basis "$out"
cp "$out/uastc_all_modes.ktx2" litegen-core/src/mesh/testdata/ktx2/
zstd -q -f --ultra -22 --long=27 \
  -o litegen-core/src/mesh/testdata/ktx2/uastc_all_modes.rgba.zst "$out/uastc_all_modes.rgba"
```

It is a stand-in for an encoder of ours (the "Not here yet" section below): it
supplies the non-power-of-two size and the UASTC scheme 0 case that no downloaded
file has, with UASTC alpha (modes 9 to 17).

## The references: `<name>.rgba.zst`

For each input, `<name>.rgba` is level 0, layer 0, face 0, **raw RGBA8**:
width x height x 4 bytes, rows top to bottom, no header, no padding, and no
colour conversion (the DFD's transfer function does not change a byte). The
folder holds it **compressed**, as `<name>.rgba.zst`, one Zstandard frame: the
raw files were 12,667,176 bytes (12.65 MB) together, and the `.rgba.zst` files
are 404,497 (395 KiB). The tests inflate each with `ruzstd` (the crate that
decodes Zstandard KTX2 levels, so a bad frame fails there too) and compare the
decoder's output with the raw bytes, byte for byte; the table's sha256 is of
the raw bytes, which is what a reference *is*.

The tool is the Basis Universal transcoder that ships with three.js: the
`KTX2File` class of `basis_transcoder.js` and `basis_transcoder.wasm`, as
installed in this repository's `scripts/mesh-verify`
(`node_modules/three/examples/jsm/libs/basis/`). It transcodes to `RGBA32`
(target 13), the same call three's own `KTX2Loader` makes to get RGBA pixels.

- **Version.** The module exposes no version string. What identifies it:
  three 0.183.2 (tag `r183`); `basis_transcoder.js` 57,529 bytes, sha256
  `8478b5b6d6b74e7d3082b89f6417321d8d1dc0307f2b30d4484bb11b441696a1`
  (git blob `6f02a4ac8504ea07512bd1106e7fe98012aecd6a`);
  `basis_transcoder.wasm` 527,333 bytes, sha256
  `6cf17dc889352c42e9acf8897107978d127005fe3386c36a0e3845e27967630a` (git blob
  `bc64682561438128604ac550da774f8cc6617460`). Both blobs are the ones at tag
  `r183` of mrdoob/three.js, and the last commit to touch either is three's
  `302240259d`, "deps(basis): Upgrade to Basis Universal v1.50 (#29418)"
  (2024-09-16). So the oracle is **Basis Universal v1.50**, not the later
  commit `073eb6a0a34a` that the design names. ETC1S and UASTC LDR 4x4 to RGBA32
  are fixed by the format, so a later transcoder is expected to give the same
  bytes, but that has not been cross-checked: it waits for a build of Binomial's
  `basisu` or KTX-Software (`basisu -unpack`, `ktx extract --transcode rgba8`),
  which is not allowed yet.
- **Node** v24.15.0 ran it.
- **Script:** `make_reference.mjs` in this folder. It loads the two files above
  (read only; nothing is copied into `node_modules`), checks every fact the
  transcoder reports against the file's own bytes, transcodes twice and requires
  the same bytes, then writes the `.rgba`. It prints each input's and each
  reference's sha256, and exits non-zero on any disagreement. It is run by hand
  and is not run in CI. To regenerate, from the repository root, write the raw
  files to a scratch folder and compress them with the `zstd` command line tool
  (v1.5.7 made the committed files; `-22` is its slowest level and `--long=27`
  its widest window, and each frame still names a window of its file's own
  size, which is 4 MiB at most). Another zstd version may write other bytes
  for the same reference, which is why the table lists the raw sha256:

  ```sh
  out=$(mktemp -d)
  node litegen-core/src/mesh/testdata/ktx2/make_reference.mjs \
    scripts/mesh-verify/node_modules/three/examples/jsm/libs/basis \
    "$out" litegen-core/src/mesh/testdata/ktx2/*.ktx2
  for f in "$out"/*.rgba; do
    zstd -q -f --ultra -22 --long=27 \
      -o "litegen-core/src/mesh/testdata/ktx2/$(basename "$f").zst" "$f"
  done
  ```

  To check one: `zstd -d -c <name>.rgba.zst | shasum -a 256` gives the raw
  sha256 below.

| reference | raw bytes | raw sha256 | `.zst` bytes |
|---|---:|---|---:|
| `FlightHelmet_Materials_LensesMat_Normal.rgba.zst` | 4,194,304 | `04bf3751b53ca9bd427ae863c5acf4ba65e012d24c48c77d6c80a9b933712744` | 523 |
| `FlightHelmet_Materials_LensesMat_BaseColor.rgba.zst` | 4,194,304 | `adcedb19840e410421215cacf41ab6b6608d39bf4edb317993c66eeb042737eb` | 123,451 |
| `Rib_N.rgba.zst` | 4,096 | `069f668759fb1b5d9940ddf28ed7ebecc6cf9119ff4f502d5f6966ba35a01423` | 79 |
| `Dot_C.rgba.zst` | 65,536 | `f84eda501eccd963e318ecaec549194262a654876887cbc1574a004386ec380f` | 301 |
| `StainedGlassLamp_grill_basecolor-alpha.rgba.zst` | 4,194,304 | `3f908d88e4ba72946886514d6ec73418ca0eceee6f7b6a9b93e48bbe3eb4df48` | 271,708 |
| `uastc_all_modes.rgba.zst` | 14,632 | `5bf8671249c817e162d1550c4b682e5203d0b3f902be7410a9363c182ac194fc` | 8,435 |

The six raw references are 12,667,176 bytes; the six `.rgba.zst` files are
404,497. The inputs (the five downloaded files and `uastc_all_modes.ktx2`) are
318,221 bytes.

## Not here yet: our own encodings

The design also wants textures made from `fixtures::quadrant_png()` and
`bands_png()` and encoded as ETC1S, ETC1S with alpha, UASTC and UASTC + Zstd,
plus one uncompressed `R8G8B8A8` KTX2. They need an encoder (Binomial's
`basisu` or KTX-Software's `toktx`), and building one is not allowed yet, so
they are deferred. The uncompressed path is tested from hand-built bytes in
`ktx2.rs` meanwhile.

Zstandard is covered by the two real UASTC files above (frames of `libzstd`,
whose 365 and 103 bytes inflate to 1 MiB and 1 KiB), by the `.rgba.zst`
references themselves (`zstd` 1.5.7 frames, inflated by `ruzstd` in every test
that reads one), and by frames the tests write block by block. A corpus of
`libzstd` frames at every level, compared with the reference decoder's output
byte for byte, is a later task's.
