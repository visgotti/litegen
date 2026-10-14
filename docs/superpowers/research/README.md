# Format research behind `litegen-core/src/mesh/`

The two notes here were written on 2026-10-12, before the FBX, USD/USDZ, 3MF and
Collada codecs were built. They hold what each format actually requires, checked
against real files and tools, with the sources for every claim. Code comments that
say "research notes §…" mean these files.

| Note | Covers |
|---|---|
| [`research-fbx.md`](research-fbx.md) | Binary FBX 7400/7500 written without the Autodesk SDK: container, objects, geometry layers, skins, blend shapes, materials, textures, animation curves, axis and unit settings, and what each reader (SDK tools, Blender, Assimp, three.js, ufbx) accepts. Also covers licensing. |
| [`research-usd-3mf-dae.md`](research-usd-3mf-dae.md) | USD (USDA grammar, the USDC crate encoding, USDZ packaging, UsdPreviewSurface, UsdSkel, `usdchecker --arkit`), 3MF Core and Materials, and COLLADA 1.4.1, each mapped to and from glTF 2.0. |

Each claim carries a tag that says how it was checked. The tags are defined at the
top of each note. In short:

- **Checked locally with a real tool or real bytes:** `[V]` and `[L]`.
- **Taken from a primary source** (a spec or upstream source, cited inline): `[S]`, `[3js]`, `[assimp]`, `[ufbx]`, `[SDK-doc]`.
- **Not confirmed:** `UNVERIFIED`.

## Files

The notes were written in a temporary working directory, and their paths still name
it:

- `scratchpad/fbx/…` is now [`fbx/`](fbx/).
- `scratchpad/usd3mfdae/…` is now [`usd3mfdae/`](usd3mfdae/).

Only the scripts we wrote and the hand-written inputs were kept:

- **Scripts:** `scripts/`, `node/` and `usd3mfdae/web/test.html`.
- **Inputs:** `usd3mfdae/usd/*.usda`, `usd3mfdae/crate/*.usda` and the USD test textures.

Some things were left out on purpose:

- **Third-party material:** downloaded pages, third-party sources, sample models and licence texts. The notes cite these by URL and version.
- **Generated output:** renders, probe files, and `.usdc`, `.usdz` and `.3mf` builds. The scripts and the tools named in each note regenerate them.

Each script states its usage in its header. All of them use only the standard
library.

- **Python:** `python3 -I <script> …`.
- **`node/*.mjs`:** these import three.js from `dashboard/node_modules/three`, so run
  `npm install` in `dashboard/` first.
- **`usd3mfdae/web/test.html` with `scripts/probe.sh`:**
  1. Link `usd3mfdae/web/three` to `dashboard/node_modules/three`.
  2. Serve `usd3mfdae/web/` on `127.0.0.1:8765`.
  3. Run `scripts/probe.sh "<query>"`, which drives headless Chrome.
- **`scripts/negtests.sh` and `scripts/chk.sh`:** these need Apple's USD Tools
  (`usdchecker`).

`usd3mfdae/scripts/make_ref_dae.py` also writes the hand-built Collada test files in
`litegen-core/src/mesh/testdata/dae/research/` (see the README there).

## What these notes are not

They are research, not the spec of what litegen does. Two other documents cover that:

- **What the converter does today:** [`../3d-format-conversion.md`](../3d-format-conversion.md).
- **Where the code departs from these notes or from the formats:**
  [`../2026-10-12-mesh-conversion-review.md`](../2026-10-12-mesh-conversion-review.md).
  The FBX colour space is one case where the notes were right and the code is not.
