# Mesh gaps: specs and plans for the twelve open items — overview

Written 2026-11-04 at the user's request: "a plan and implementation spec for each of
these, that includes what we must test to validate and ensure that it's fully
implemented without question". The twelve items are the "doable, not done" list of the
litegen 3D conversion matrix page, which litegen-a3 built from
`docs/superpowers/3d-format-conversion.md`, the 2026-11-01 mesh ledger and the Nov 3
known-gaps handover. Each item has a design (this folder) and a plan
(`docs/superpowers/plans/`). This page is the index, the shared rules, and the shared
bar for "done".

## The twelve

| # | Item | Size | Spec | Plan | Decision | Status |
|---|---|---|---|---|---|---|
| 1 | Multi-file downloads | M | [design](2026-11-04-mesh-multi-file-downloads-design.md) | [plan](../plans/2026-11-04-mesh-multi-file-downloads.md) | D1: zip, opt-in | **done** (litegen-05), not committed |
| 2 | Draco and meshopt glTF input | L | [design](2026-11-04-mesh-compressed-gltf-input-design.md) | [plan](../plans/2026-11-04-mesh-compressed-gltf-input.md) | D2: meshopt std-only, Draco via `draco-core` | **done** (litegen-b2), not committed; one Draco-limits follow-up open (B41, in its plan's status block) |
| 3 | KTX2 (Basis) textures to PNG | L | [design](2026-11-04-mesh-ktx2-textures-design.md) | [plan](../plans/2026-11-04-mesh-ktx2-textures.md) | D3: `ruzstd`, fully tested | **done** (litegen-fc), not committed; Miri and a `basisu` build wait on the user (its plan's status block) |
| 4 | Glass, clearcoat and other material layers | M | [design](2026-11-04-mesh-material-layers-design.md) | [plan](../plans/2026-11-04-mesh-material-layers.md) | — | **done** (litegen-b2), not committed |
| 5 | Lights and cameras | M | [design](2026-11-04-mesh-lights-cameras-design.md) | [plan](../plans/2026-11-04-mesh-lights-cameras.md) | — | **done** (litegen-fc), not committed |
| 6 | USD: USDC output, several clips, unlit | M | [design](2026-11-04-mesh-usd-usdc-clips-unlit-design.md) | [plan](../plans/2026-11-04-mesh-usd-usdc-clips-unlit.md) | D4: `usdc` public; `usdz_root`, `usd_clips` options | **done** (litegen-fc, 2026-11-10), not committed; the user skipped the AR Quick Look device check, so USDZ's `usd_clips` default stays `first` |
| 7 | FBX: animation layers, ASCII output | S | [design](2026-11-04-mesh-fbx-layers-ascii-design.md) | [plan](../plans/2026-11-04-mesh-fbx-layers-ascii.md) | D5: ASCII opt-in | can start (all tasks) |
| 8 | Collada morph targets in output | S | [design](2026-11-04-mesh-dae-morph-output-design.md) | [plan](../plans/2026-11-04-mesh-dae-morph-output.md) | D6: `dae_morphs=keep` opt-in | **done** (litegen-fc), not committed; one follow-up open (DM4, a SceneKit clip-name opt-in, in its plan's status block) |
| 9 | Big-endian PLY input | S | [design](2026-11-04-mesh-ply-big-endian-design.md) | [plan](../plans/2026-11-04-mesh-ply-big-endian.md) | — | **done** (litegen-5c), not committed |
| 10 | Points and lines | M | [design](2026-11-04-mesh-points-lines-design.md) | [plan](../plans/2026-11-04-mesh-points-lines.md) | — | **done** (litegen-fc), not committed |
| 11 | More than two UV sets in USD and Collada | S | [design](2026-11-04-mesh-uv-sets-usd-dae-design.md) | [plan](../plans/2026-11-04-mesh-uv-sets-usd-dae.md) | — | **done** (litegen-37, closed by litegen-b2), not committed |
| 12 | The six Oct 30 review leftovers | S | [design](2026-11-04-mesh-review-leftovers-design.md) | [plan](../plans/2026-11-04-mesh-review-leftovers.md) | — | **done** (litegen-a3 [1a9a54]), not committed |

The status is as of 2026-11-09. Each session keeps its ledger in
`.superpowers/sdd/2026-11-04-mesh-<item>/`. A finished item's plan opens with a status
block giving where the build differs from the plan, and the files to commit.
**litegen-d0 holds the frontend/SDK role** (litegen-d8 has ended). The
[handoff](../plans/2026-11-04-mesh-gaps-handoff.md)'s "Roles that moved" says how the
role works. Send d0 every OpenAPI-visible shape first. It regenerates the SDKs and landing once each tree builds,
batching items 3, 6, 7 and 8 where they land together. A builder check (a fresh reviewer per design) has read every
spec, and its findings are folded in. The research notes behind them are
[`research-codecs.md`](../research/research-codecs.md) and
[`research-consumers.md`](../research/research-consumers.md), in the tracked research
folder.

### What can start now

Items 1, 2, 3, 4, 5, 8, 9, 10, 11 and 12 are done and hold no files (item 4 closed
2026-11-05, tree 6879167; item 5 closed 2026-11-07, tree f84419f; item 2 closed 2026-11-08,
tree 6fbad32; item 10 closed 2026-11-08, tree fe679b6; item 3 closed 2026-11-09, tree
e8270b9; item 8 closed 2026-11-09, tree 7e02650).
Item 2 leaves one follow-up, which any session can take: the rest of Task 4's Draco limits
(B41). That's a faces-per-point pre-check (B39), a RED test for the decoded-bytes ceiling,
the ceiling's final numbers in draco.rs's module doc, and the fix round's scoped
re-review. Its plan's status block says what's in and what's open.

**Item 3 is done (litegen-fc, 2026-11-09).** Every production path transcodes KTX2 to PNG
by default, and the opt-in `ktx2_textures=keep` keeps it in glb and gltf. Two parts of
D3's "fully tested" bar wait on the user, as its plan's status block says: one Miri run
(nightly isn't installed) and the `basisu` build that would cover UASTC with alpha.

**Item 8 is done (litegen-fc, 2026-11-09).** The opt-in `dae_morphs=keep` writes Collada
morph controllers in the form SceneKit was measured to play; the default still bakes them.
Its plan's status block has the proof, the rulings DM1–DM23 and one follow-up (DM4).

These can start every task:

- **Item 6:** USDC output, clips and unlit. Item 4's Task 3 is in, so Task 0 changes by
  hand the `Value::Str` matches it added (Ruling A17).
- **Item 7:** FBX layer blending, the ufbx reference numbers, ASCII output.

Item 10 is in the tree. A new fixture that holds lights or cameras together with points
or lines would mismatch in the camlight judges (item 10's Ruling I10-R18).

Shared files still go one session at a time, in the handoff's "Shared files" order, with
regions agreed by message first, as items 1 and 12 and then items 2 and 3 did. Every
item's last task takes a harness turn.

## What the list gets wrong today

These were checked in the tree on 2026-11-04, and each spec starts from them:

- **Item 12 is half done.** These are already fixed, with tests:
  - `height_m` on a flat mesh is reported (`options_that_cannot_be_applied_are_named_on_the_asset`);
  - renames now run before de-duplication, and the dead `joint_renames` path is gone
    (`names_stay_unique_after_joint_naming`, 2026-11-03).

  The resampling cap is half there: the grid is capped per clip, but channels × grid
  isn't. Joint names across a chained animate and glTF `skin.skeleton` are open.

  **Closed on 2026-11-04 (litegen-a3 [1a9a54], not committed):** all six leftovers, a–f.
  a, c and d were re-proved by their tests. b, e and f were fixed: a chained animation
  keeps its rig's joint names; written glTF names the joints' closest common root as
  `skeleton`; and a rekey past the key or value budget is declined before anything is
  allocated. The plan's status block lists what differs from the plan and the files to
  commit.
- **KTX2 isn't "kept in glTF output" on any production path.** Delivery and the
  converter run `normalize::transcode_textures`, which drops KTX2 from every output,
  GLB included. Only the library's `write_scene` keeps it.
- **glTF cameras are dropped without a warning.** `glb.rs` has no camera handling at
  all, while `KHR_lights_punctual` is named in the warnings. Item 5 fixes it. The
  warning alone could ship first.
- **The model3d-provider skill says `KHR_mesh_quantization` is refused.** `glb.rs`
  reads it (`CARRIED_EXTENSIONS`). Item 2 corrects the skill.
- **`mock/partial-3d`'s "undecodable" GLB only declares Draco as required**
  (`providers/model3d/mock.rs`, `generate_undecodable_glb`). Once Draco reads, it
  decodes, so item 2 moves the mock to a construct litegen will never read.

## Order

1. **First, independent and small:** 12, 9, 11. No decisions are needed, and each is a
   few days or less.
2. **Scene-model additions, one at a time:** 4 → 5 → 10. Each adds to `scene.rs`,
   `Capabilities` and `LossKind`, every codec, `fixtures.rs` and the harness. Done in
   parallel, they would collide in the same files and the same capability struct.
3. **Writers:** 6 (after 11 and 4, which also touch `usd_write.rs`), 7 and 8.
4. **Decoders:** 2 (meshopt first, then Draco through `draco-core`), then 3 (with
   `ruzstd`).
5. **API:** 1 can run any time after D1. It's OpenAPI-visible, so it goes through the
   frontend/SDK session. Batch its regeneration with any other OpenAPI change ready at
   the same time (item 6's `usdc` format name, item 7's `fbx_encoding`, item 8's
   `dae_morphs`, the new loss kinds' header text).

## Rules every plan carries

- **`mesh/` stays a standalone library** (`.claude/rules/mesh-standalone.md`):
  - no `crate::`, and no `super::` that climbs out of the module;
  - std, plus only `serde_json` (glb.rs, matrix_tests.rs), `miniz_oxide` (zip.rs,
    fbx.rs), `quick_xml` (xml.rs) and `image` (normalize.rs);
  - a new `.rs` file goes in `mod.rs` and in `MODULE_FILES` in `isolation_tests.rs`;
  - anything litegen calls is `pub`;
  - no I/O, no async;
  - fixtures live in `src/mesh/testdata/`, loaded with `include_bytes!`.

  **A new crate is the user's decision, never a drive-by** (D2, D3).
- **No commits on this laptop, and nothing left staged.** The user copies the folder and
  commits elsewhere. If asked, commit with pathspecs only.
- **Cargo:** ask before running it, never while another session's cargo runs, and use
  `CARGO_INCREMENTAL=0` when disk is short. Prefix PATH with
  `/opt/homebrew/bin:/usr/local/bin` in a fresh shell.
  - Several test filters go after `--`: `cargo test --lib -- mesh::usd mesh::dae`.
    Cargo itself takes only one filter, and `cargo test --lib mesh::usd mesh::dae`
    fails. Every plan here uses the `--` form (fixed 2026-11-04).
  - To wait for another session's cargo, loop while `pgrep -qx cargo || pgrep -qx rustc
    || pgrep -qx clippy-driver` succeeds. `pgrep -f cargo` also matches other sessions'
    waiting shells, and two such waiters deadlock.
  - Before a full gate, a release build or a harness run, message every session editing
    `src/mesh/`. Wait for each one's "green", meaning it compiles and no test is red by
    design. Send "done" afterwards.
- **OpenAPI-visible changes:** send the exact shapes to the frontend/SDK session first.
  That session regenerates once the tree builds (`sdks/scripts/regen-all.sh`, then
  `node apps/landing/scripts/sync-openapi.mjs`). Then regenerate `docs/mcp/tools.md`
  (`cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`, from `litegen-core/`).
- **Opt-in options reuse item 1's plumbing** (done 2026-11-04). Never copy it:
  - `option_needs_keeping(param, what, model)` in `api/handlers/mod.rs` is the 400 that
    names `param`. It fires in `generate_3d`'s keep-nothing block;
  - the request field → `DeliveryPlan` → write pattern of `DeliveryPlan.packages`, with
    a serde default and skipped when off;
  - `write_keyed(scene, target, self_contained, key_fps, plan)` in
    `proxy/model3d_deliver.rs` is where a new `WriteOptions` field gets threaded;
  - `MeshFormat::writes_sidecars()` tells the multi-file containers from the rest.
- **Every behaviour change has a test that failed first.** The ledger records RED, then
  GREEN, with the test's name.
- **No mutation testing (the user's rule, 2026-11-09, in the global `CLAUDE.md`).** Plant no
  deliberate bug to show a test can fail: no `NEGCTL` mutant, by hand, by tool or by a
  subagent. Every negative-control step in these specs and plans is void. Prove a new test
  red before the change and green after, and for a test of existing behaviour, say what
  regression it would catch. If any file is ever restored after a temporary edit, rewrite
  it rather than `mv` it back, since an old mtime keeps the old build, and `touch` it.
- **Third-party files stay out of the repo** unless their licence allows otherwise.
  litegen's own fixtures, written by a third-party tool, may be committed in
  `src/mesh/testdata/` with a README line naming the tool and version.
- **Ledgers go in `.superpowers/sdd/<date>-<item>/`,** never only in `/private/tmp`.
  That includes the harness report's `summary` block.

## What "done" means for every item

Each spec lists its own tests. These hold for all of them, and an item isn't done until
every line is true and recorded in its ledger:

1. **RED → GREEN.** Every test the spec names exists, was seen failing before the
   change, and passes after.
2. **~~Negative controls.~~** Void since 2026-11-09 (the user banned mutation testing).
   Instead, every test the spec names was seen red before the change (1). A test of
   behaviour that already existed says which regression it would catch.
3. **Unit and matrix.** `cargo test --lib mesh::` passes, and `scene_matrix_tests`
   covers the new content in every pair and both write modes.
4. **Full gate:**

   ```sh
   export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; cd litegen-core
   CARGO_INCREMENTAL=0 cargo test --lib -- --skip scale_animated_ancestors_are_found_once_per_chain --skip many_distinct_key_time_arrays_write_in_linear_time --skip composites_are_mixed_once_and_base_indices_cannot_multiply_primitives
   CARGO_INCREMENTAL=0 cargo test --lib -- scale_animated_ancestors_are_found_once_per_chain many_distinct_key_time_arrays_write_in_linear_time composites_are_mixed_once_and_base_indices_cannot_multiply_primitives
   CARGO_INCREMENTAL=0 cargo test --tests            # integration suites; with LITEGEN_PG_TEST_URL for Postgres when the item touches the API
   CARGO_INCREMENTAL=0 cargo clippy --all-targets --all-features -- -D warnings
   ```

   The three load-sensitive mesh tests run alone, as on 2026-11-03.
5. **Standalone.** `mesh::isolation_tests` passes, and a read of the diff confirms no
   `crate::`, no escaping `super::`, and no crate outside the list.
6. **Render harness:** `cargo run --release --example mesh_matrix -- <dir>`, then
   `node scripts/mesh-verify/verify.mjs <dir> <report>`, with:
   - 0 failures on the fixture set;
   - every refusal being the expected scene's own;
   - every new skip giving a reason that cites the reader's source line;
   - the item's own third-party set run with `--import`.

   Copy the report's `summary` into the ledger.
7. **Hostile input.** Every reader path the item adds gets:
   - a truncation sweep, every prefix;
   - a bit-flip sweep, every bit of the first 4 KiB plus a seeded sample of at least
     10,000 elsewhere, seed recorded.

   Neither may panic, and each case must finish in under a second in release.
   Allocations are bounded by a named budget, charged before allocating.
8. **Docs:**
   - `3d-format-conversion.md` (the codec's section and §9);
   - the `mesh/mod.rs` lossiness table;
   - the codec's module doc;
   - the model3d-provider skill wherever it states the old behaviour;
   - the matrix page data (send litegen-a3 the changed cells; that session owns
     `3d-matrix/build.mjs`).
9. **API-visible items** (1, 6, 7, 8, and the loss-kind text): the openapi tests, the
   SDK suites after regeneration, the MCP tool and doc tests, and dashboard vitest and
   Playwright where the UI shows the change.

## Shared additions, and which item makes each

| Addition | Made by | Used by |
|---|---|---|
| `fixtures::layered()`: clearcoat, transmission + volume, sheen, specular, ior, iridescence, anisotropy | 4 | 4, 6 |
| `fixtures::staged()`: a directional, a point and a spot light, a perspective and an orthographic camera, under animated nodes | 5 | 5 |
| `fixtures::wire()`: lines, a line loop, points with colours, beside a triangle mesh | 10 | 10 |
| `fixtures::multiuv()`: four uv sets, with a texture on set 3 | 11 | 11, 1 |
| `fixtures::clips()`: one skinned rig with three named clips of different lengths | 6 | 6, 7, 8 |
| Harness `lit` mode: PBR with a fixed environment and the file's lights | 4 | 4, 5 |
| Harness `camera` mode: render through the file's first camera | 5 | 5 |
| Harness point and line rendering | 10 | 10 |
| Harness zip check (each case's package zipped by `zip::write_package`, unpacked, byte-compared with the case folder; the harness already renders packages) | 1 | 1 |
| Harness `usdc` row and column, and the `usdcat` text comparison | 6 | 6 |
| Harness per-variant USD rendering | 6 | 6 |
| `LossKind` `lights`, `cameras`, `points`, `lines` (wire strings, additive) | 5, 10 | the converter header text, the SDK doc comments |
| The opt-in option plumbing (a request field, then `DeliveryPlan`, then the writer's `WriteOptions`; a converter query parameter; the `option_needs_keeping` refusal and its OpenAPI text) | 1 (done; "Rules every plan carries" names the functions) | 3, 6, 7 and 8, which reuse it rather than add their own |

## Decisions (settled with the user, 2026-11-04)

The user's answers: "do opt in … make sure users can decide and have options"; for D3
"do whatever is most stable … make sure it's fully tested completely"; and "anything
after that do what ur recommendation is always".

- **D1 (item 1): zip packages, opt-in.** The converter's `package=file|zip` and the 3D
  request's `output_packages: true`, as asset `kind: "package"`. The default stays one
  file.
- **D2 (item 2): meshopt std-only; Draco through `draco-core =2.3.3`.** It's pinned
  exactly and confined to `mesh/draco.rs` behind one function. It's held to the D3 bar:
  a differential test against Google's own `draco_decoder` over every committable
  sample and a corpus of our fixtures encoded at many settings, the hostile-input
  sweeps, `cargo audit`, and absolute decode limits. Under the mesh-standalone rule,
  `ALLOWED` gains it for `draco.rs` only.
- **D3 (item 3): `ruzstd =0.9.1`.** It's the stable choice: years in production,
  77 M downloads, fuzzed upstream. It's pinned exactly, confined to `mesh/ktx2.rs`, and
  "fully tested":
  - a differential test against the reference `zstd -d` over a generated corpus
    covering every frame and block type;
  - the bit-exact KTX2 tests;
  - the hostile-input sweeps, with the output capped at the expected length;
  - a Miri run of the Zstandard tests over its `unsafe` ring buffer;
  - `cargo audit`.
- **D4 (item 6): `usdc` becomes a public format.** USDZ keeps its USDA root by default,
  and `usdz_root=usdc` opts in to a crate root.
- **D5 (item 7): ASCII FBX is built, opt-in** (`fbx_encoding=ascii`). Binary 7400 stays
  the default.
- **D6 (item 8): `dae_morphs=keep`, opt-in,** on the converter and on 3D requests.
  Baking stays the default.

## Options users choose (opt-in)

Every output behaviour where reasonable callers want different things is a parameter,
not a hidden policy. Each one is opt-in:

- **The default is today's output, byte for byte,** and each item tests that.
- **It's offered everywhere litegen writes a mesh:** as a query parameter on `POST
  /v1/utils/models3d/convert`, and as a field of 3D generation requests that every 3D
  model accepts, because litegen applies it on delivery. That's the pattern
  `fbx_color_space` already follows.
- **It's recorded in the row's `DeliveryPlan`,** so the client's poll and the poller
  deliver alike.
- **A request that keeps nothing (`keep_outputs: false`) and sets one** to a
  non-default value is refused, 400 `option_needs_keeping`, naming it. litegen hands
  out the vendor's own files then, so the option couldn't apply, and it isn't dropped
  silently.
- **Its OpenAPI text says who reads what,** for example "Blender's legacy importer
  refuses ASCII FBX".

| Option (converter query / request field) | Values, default first | Item |
|---|---|---|
| `package` / `output_packages` | `file` / `false`, then `zip` / `true` | 1 |
| `ktx2_textures` | `png` (KTX2 transcoded for every container), then `keep` (glb and gltf keep the KTX2 with `KHR_texture_basisu`; other containers still get PNG) | 3 |
| `usdz_root` | `usda`, then `usdc` | 6 |
| `usd_clips` | `all` (Apple's variant layout), then `first` (today's single clip) | 6 |
| `fbx_encoding` | `binary`, then `ascii` | 7 |
| `dae_morphs` | `bake`, then `keep` | 8 |

`usd_clips` is the one whose USDZ default depends on evidence. It's `all` for USDA and
USDC. For USDZ it's `first`: the user skipped item 6's AR Quick Look check on 2026-11-10,
and it flips to `all` only if a later check passes. Callers can pick either way.

Later options considered and left out of these twelve, each a follow-up of its own:
- MaterialX `gltf_pbr` materials in USD, which carry every material layer;
- RealityKit's MaterialX unlit node, as a "RealityKit profile";
- a meshopt-compressed glTF output.

## Adjacent work, not on this list

- The three mesh tests that fail under full-suite load and pass alone (fbx
  `scale_animated_ancestors_are_found_once_per_chain`, mesh
  `many_distinct_key_time_arrays_write_in_linear_time`, threemf
  `composites_are_mixed_once_and_base_indices_cannot_multiply_primitives`).
- Proof gaps from the matrix page:
  - the Khronos and third-party USD sets last passed on Oct 12;
  - the live fal run (`tests/live_model3d.rs`) needs a `FAL_KEY`;
  - nobody has opened litegen output in Unity, Blender, AR Quick Look or a slicer.
- Uploading a package (zip) to the converter: the input side of item 1.
