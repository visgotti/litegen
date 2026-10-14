# 3MF test vectors

Files written by tools other than litegen, read by `threemf.rs`'s tests to prove
the reader handles what other producers emit, not only what our writer does.

## assimp_textured.3mf

- **What:** litegen's `textured` fixture (`fixtures::textured()`), written as GLB
  by litegen's own writer, then exported to 3MF by assimp: the 3MF a widely used
  independent converter makes from our content.
- **Command** (assimp 6.0.5 from Homebrew, 2026-10-12):

  ```
  cargo run --example mesh_matrix -- out --fixtures textured --formats glb
  assimp export out/textured/src/glb/model.glb assimp_textured.3mf
  ```

  SHA-256 `283bcd97be0dff5b172fa441d435feef2e94a5ff7f695d6f86928f9997b89e87`.
- **Licence:** the content is litegen's own fixture; assimp (BSD-3-Clause) claims
  nothing in the files it writes.
- **What it exercises** — each a departure from 3MF Core 1.4 that assimp 6's
  `code/AssetLib/3MF/D3MFExporter.cpp` makes on every export:
  - a ZIP64 end record in a 1.8 KB archive, and data descriptors on every entry
    (3MF consumers "MUST support ... the PKWARE ZIP64 extension", Core §1.1);
  - `<metadata>` inside `<resources>`, its names un-namespaced;
  - one build item per mesh with no transform: assimp writes only the root's
    children and drops their placement, writes `unit="millimeter"` over metre
    values, and does not turn Y-up into Z-up;
  - display colours written as linear values times 255 (`#e54c19` is linear
    0.9/0.3/0.1), where 3MF colours are sRGB;
  - no texture coordinates and no image: its 3MF writer keeps base colours only.

  The reader takes the file at its word, so the model reads back 1000 times
  small, lying on its back and darker than the source — which is exactly what
  the file says.
