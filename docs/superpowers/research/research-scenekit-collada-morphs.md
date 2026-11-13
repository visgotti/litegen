# What SceneKit plays: Collada morph controllers, their weight animation and a skin over them

Research date: 2026-11-09, Task 0 of mesh-gaps item 8
(`docs/superpowers/specs/2026-11-04-mesh-dae-morph-output-design.md`), the spike that decided whether the Collada writer
can keep morph targets on request (`DaeMorphs::Keep`, the converter's `dae_morphs=keep`). No product code was touched.
`litegen-core/src/mesh/dae.rs`'s module doc and `docs/superpowers/3d-format-conversion.md` §7 state what came of it;
this note holds the measurements behind them.

Every claim below was measured on SceneKit itself: a hand-written Collada file was loaded, drawn and compared with a
control that has no controller at all, the morph (and skin) applied by hand. Where the first form tried did not play, the
variant that does is named in the table.

**Where the rest is.** The pictures and contact sheets, the generator and runner scripts, the control files and the
raw result tables stayed in the item's ledger folder,
`.superpowers/sdd/2026-11-04-mesh-dae-morph-output/spike/` (about 10 MB, 1,279 files). Git ignores that folder and it is
not part of the repository, so this note keeps the tables and conclusions and the commands to reproduce, and nothing
here depends on the files being there. The renderer patch the measurements needed is committed:
`scripts/mesh-verify/scenekit/render.swift`.

## What was probed

- **SceneKit:** the system's, macOS 26.6.2, Swift 6.3.3. Xcode's scene editor, Godot, Unity and SketchUp were not
  tried. Godot's reading is taken from its source in the spec, not measured.
- **Renderer:** `swiftc -O` of a byte-identical copy of `scripts/mesh-verify/scenekit/render.swift` as it was before
  this item (sha-256 `6e6f20f5…`), and of a copy with the morpher patch described under "The renderer drives only the
  node's animations" below. Both run headless and draw 256 x 256 pictures.
- **Cameras**, aimed at the origin, field of view 35, near 0.1, far 100: `front` (0, 0.8, 5); `side` (5, 1, 1.4), which
  sees the bulge's profile and the arm's bend; `ob`, oblique (3.2, 2.2, 3.6).
- **Judges:** `silhouette`, the harness's coverage compare (`verify.mjs`, limit 1 %), and `lights`, a white Lambert
  surface under the file's directional light `sun`, compared with `compareNear` at 24/255 and a 2 % limit. Every ratio
  is a share of the union of covered pixels, in percent. A right control scores 0.00 everywhere.
- **Inputs**, written from the `morph` fixture's numbers: a 10 x 10 grid of size 2 as the base; two targets, a bulge
  and a twist; default weights (0.25, 0); a clip `Breathe` at keys 0, 0.8 and 2.0 with values (0, 0), (1, 0.6) and
  (0.2, 1). The skin case adds joints `root` and `arm`, the arm turning 45 sin(pi t) degrees, keyed every 0.02 s, with
  identity node, bind-shape and inverse-bind matrices.
- **Sample times**, t0 to t5:

  | t | time (s) | bulge | twist | sum | arm angle (deg) |
  |---|---|---|---|---|---|
  | t0 | 0 | 0 | 0 | 0 | 0 |
  | t1 | 0.34 | 0.425 | 0.255 | 0.68 | 39.43 |
  | t2 | 0.72 | 0.9 | 0.54 | 1.44 | 34.67 |
  | t3 | 1.16 | 0.76 | 0.72 | 1.48 | -21.68 |
  | t4 | 1.66 | 0.4267 | 0.8867 | 1.313 | -39.43 |
  | t5 | 2.0 | 0.2 | 1.0 | 1.2 | 0 |

  Four of the six times sum past 1, so they test whether NORMALIZED is clamped (it is not, row 4 below).
- **Controls:** files with no controller, the morph applied by hand: the rest pose, each of t0 to t5, the same with the
  base's normals only (`flatn`), and for the skin case the full skin(morph(base)), the skin alone and the morph alone.
- **Sanity:** litegen's baked output of the same morph (`DaeMorphs::Bake`, today's writer) draws 0.00 % apart from the
  rest control on all three cameras; no picture is blank (the fewest drawn pixels is 4,410); every file is well-formed.
  The files were not checked against the 1.4.1 XSD then; the harness does that now (`xmllint`, pinned by sha-256).

## Findings

The branch taken is the first row of the spec's decision table: SceneKit plays morph controllers, a weight animation
and a skin over a morph, so `Keep` ships as designed (NORMALIZED, absolute targets, `POSITION` and `NORMAL` inside
`<vertices>`).

| # | Question | Answer | Evidence |
|---|---|---|---|
| 1 | A static `<morph method="NORMALIZED">` with a full target `<geometry>` each, an `IDREF_array` and a `float_array` of default weights: does SceneKit show the targets at those weights? | **Yes.** SceneKit read `MORPHER targets=2 mode=normalized weights=[0.25,0.0]` | Rest against the hand-applied rest control: 0.00 on all six judges, pixel-identical at tolerance 0. Nearest wrong control (weights 0, 0): 5.55 side, 1.39 front, 1.52 oblique; the other states 7.58 to 30.68 |
| 2 | The same as `method="RELATIVE"` with offset targets | **Yes**, identical on every judge. SceneKit read `mode=additive`; the default weights are honoured | The numbers of row 1 |
| 3 | Plus a weight animation: one channel per weight, a key at 0, target `sheet-morph-weights(i)` (the MORPH_WEIGHT `<source>`'s id) | **Plays**, on the clip route (`clip: "Breathe"`) | 0.00 on all six judges at every sample time, t1 to t4 between keys and from t2 with the sum above 1. Nearest wrong control (side silhouette): t0 against rest 5.55; t1 against rest 7.58; t2 against t3 9.97; t3 against t2 9.97; t4 against t5 6.95; t5 against t4 6.95. SceneKit registers the clip entries `Breathe`, `sheet-morph-w0-anim` and `sheet-morph-w1-anim`. RELATIVE is identical |
| 4 | Is NORMALIZED clamped, so the scene's additive offsets (weights summing past 1) cannot be written as base + offset? | **No.** Sums up to 1.48 (a base coefficient of -0.48) draw exactly as `base (1 - sum w) + sum w_i target_i`, so RELATIVE is not needed as a fallback | t2 and t3 of row 3 (sums 1.44 and 1.48), 0.00 on all six judges |
| 5 | The same channel with its target on the `float_array` id (`sheet-morph-weights-array(i)`), as the research file `litegen-core/src/mesh/testdata/dae/research/morph.dae` writes it (`research-consumers.md` Q5) | **Not played.** SceneKit builds empty animation groups (`duration=0.0 group of 0`), and every picture stays at the default weights | t1 to t5 against their controls: 7.58, 30.68, 26.65, 17.63, 18.21. Only the `<source>` id binds. Our reader takes both, so a round trip cannot see a wrong choice (Ruling DM9). Tested on the spike's own file only |
| 6 | A `<skin source="#sheet-morph">` over the animated morph: does the skin deform the morphed mesh? | **Yes.** SceneKit read a node with a MORPHER and a SKINNER (`bones=[root, arm]`), and it draws skin(morph(base)) at every sample time. RELATIVE is identical | 0.00 on all six judges against the full control. Ablations (clip route, side silhouette), against the morph-only control and against the skin-only control: t1 48.90 and 25.12; t2 36.25 and 49.73; t3 24.54 and 35.72; t4 41.04 and 21.82; t5 0.00 (arm at 0 degrees) and 19.56; t0 0.00 and 0.00. Nearest wrong `full` control: t0 6.51, t1 33.69, t2 33.69, t3 39.48, t4 40.18, t5 18.00 |
| 7 | Are the default weights (the `float_array`) honoured at rest, and does a weight animation's key at 0 override them from time 0, as glTF's does? | **Yes** to both | Rows 1 and 3 |
| 8 | Are normals inside `<vertices>` read and morphed? | **Yes.** Lit pictures are pixel-identical to the blended-normals control (15 pictures, 0 pixels over 2, largest delta 0), and differ from a base-normals-only control by up to 21,442 pixels over 2 (largest delta 17, mean 7.75, front t2) | The 24/255 tolerance cannot see normals; tolerance 0 can |
| 9 | What does SceneKit's own exporter write for the same morph? | The same NORMALIZED form: an `IDREF_array` of target geometries, a weights `<source>` that keeps the id `sheet-morph-weights` with its MORPH_WEIGHT param renamed `A`, and no animation | Its export of the animated file |

**Strictness.** At tolerance 0, 672 picture pairs were compared and 28 differ. 24 are the unmodified renderer's rest
pictures of the animated files (the next section). 4 are a 1/255 lit value on the skin case and its RELATIVE twin at t3,
oblique camera, in the unmodified and the patched renderer alike, with identical coverage. The lit judges alone tell some
neighbouring states apart only weakly (t0 against rest: 0.8 %), so the side silhouette plus tolerance-0 identity carry
the claims above.

## The renderer drives only the node's animations

SceneKit attaches a Collada weight animation to the `SCNMorpher`, not to the node. `render.swift` drove and removed
only `node.animationKeys`, so with the unmodified renderer:

- the **rest** picture (time `null`) shows the animation's t0 weights (0, 0), not the defaults (0.25, 0): against the
  hand-applied rest control 5.55 side, 1.39 front, 1.52 oblique, and against t0's control 0.00;
- a picture at a **time with no clip** shows the t0 weights at every time (t1 to t5 against their controls: 11.66,
  33.76, 29.66, 20.01, 18.96);
- a picture at a **time plus the clip `Breathe`** is exact.

A probe at t = 1.16 s read the weights back: rest and plain (0, 0); with the clip (0.76, 0.72); with the patch, rest
(0.25, 0) and plain (0.76, 0.72).

The fix is a patch of two hunks that also drives and removes `node.morpher`'s animation players. With it every picture of
the animated and the skinned files, on all three routes, equals its control at 0.00 on all six judges, and the rest
picture equals the default-weights control. The spike measured it as about 17 lines (`render-morpher.swift`); the
committed renderer carries the same patch in 22 lines with its comment (`scripts/mesh-verify/scenekit/render.swift`,
the `node.morpher` blocks). Gate 4a re-rendered 992 plain SceneKit pictures with it: none moved.

## Consequences for the writer and the harness

1. **The weight channel's target** is `<channel target="<morph id>-weights(i)">`, the MORPH_WEIGHT `<source>`'s id (the
   writer's `{controller id}-weights`), never the `float_array`'s id. SceneKit plays only the source id, and the reader
   takes both.
2. **The weight animation sits on the morpher**, so the harness must drive it there (the patch above).
3. **NORMALIZED with absolute targets** is the form: `base (1 - sum w) + sum w_i target_i`, unclamped.
4. **Both `POSITION` and `NORMAL` go inside `<vertices>`** of the base and of every target. The reader takes normal
   offsets only from there, and SceneKit morphs them.
5. **A skin over a morph** is `<skin source="#...-morph">`, and SceneKit deforms it as skin(morph(base)) at every sample
   time.

## Not established

macOS 26.6.2 only. Xcode's scene editor, Godot, Unity and SketchUp were not measured. A single weight above 1 or below 0
(the largest single weight was 1.0; sums to 1.48 are covered). STEP interpolation of a weight channel (LINEAR only). A
skin with non-identity bind-shape or inverse-bind matrices, or a morph under a transformed node, in the skin case.
Several nodes drawing one controller. Validity against the 1.4.1 XSD at the time of the spike (the harness checks it
now).

Established after the spike, by item 8's gate (2026-11-09, tree 7e02650): a morphed mesh of several primitives, as the
writer writes it. SceneKit drew Khronos's MorphPrimitivesTest (one mesh, two primitives) written with `dae_morphs=keep`
at rest, and each of its 21 pictures matched the expected one: silhouette at most 0.112 % apart, albedo at most 0.151 %.
The spike's own list had it as not established.

## Citations in one place

- Collada 1.4.1 specification, `<morph>` (`method` NORMALIZED or RELATIVE, `<source>`, `<targets>` with the inputs
  `MORPH_TARGET` and `MORPH_WEIGHT`) and `<skin>`; the writer's form is the one SceneKit wrote itself (row 9).
- `docs/superpowers/research/research-consumers.md` Q5 (Collada morph controllers): the earlier reading of the same
  readers, whose `morph.dae` targeted the `float_array` (row 5), and the three.js lines the module doc cites.
- `docs/superpowers/3d-format-conversion.md` §7: what each reader does with the file `Keep` writes (three.js and
  assimp refuse it; SceneKit plays it).
- `scripts/mesh-verify/scenekit/render.swift`: the renderer, with the morpher patch.

## Reproduce

The generator and runner scripts were in the ledger folder, which is not in the repository. To measure again:

1. Write the files from the "Inputs" above: the base grid, the two targets as full `<geometry>` elements with `NORMAL`
   inside `<vertices>`, the controller with the `IDREF_array` and the default-weight `float_array`, and a clip of one
   channel per weight targeting the weights `<source>`. Write the controls as plain meshes with the morph applied by hand.
2. Build the renderer twice, `swiftc -O` of `scripts/mesh-verify/scenekit/render.swift` and of a copy without its
   `node.morpher` blocks, to see the difference the patch makes.
3. Draw each file at rest and at t0 to t5, with and without the clip, from the three cameras in both modes, and compare
   each pair as the harness does (`scripts/mesh-verify/verify.mjs`).

The item's own harness reruns the same measurement on every `dae-keep` file the matrix writes:
`node scripts/mesh-verify/verify.mjs`, described in `scripts/mesh-verify/README.md`.
