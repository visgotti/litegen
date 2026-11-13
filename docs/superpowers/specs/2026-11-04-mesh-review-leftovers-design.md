# The six leftovers from the Oct 30 review — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: S.
The user asked for a spec and plan for each open mesh gap, with tests that settle "done"
beyond doubt (2026-11-04). The six items come from the deferred minors of
`.superpowers/sdd/2026-10-30-litegen-pixelgenie-build/progress.md` (S2 13, S3 resample,
S3 skin.skeleton, S5 m3, m4, m8 and m14).

## Where they stand today

The tree was checked on 2026-11-04. Three of the six are already fixed, so the list the
matrix page shows is out of date:

| # | Item | State | Evidence |
|---|---|---|---|
| a | `height_m` on a flat mesh is ignored silently | **fixed** (2026-10-31 audit) | `normalize::Placement::height_skipped`. Delivery notes "height_m {h} was not applied: the mesh has no height to scale" (`proxy/model3d_deliver.rs`). Test `options_that_cannot_be_applied_are_named_on_the_asset`. |
| b | joint names are not kept across a chained animate | **open** | see below |
| c | joint renames run after name de-duplication | **fixed** (Nov 3) | the humanoid renames re-run `unique_node_names`. Test `names_stay_unique_after_joint_naming`. |
| d | a dead `joint_renames` path | **fixed** (Nov 3) | `DeliveryPlan::joint_renames` is gone (`2026-11-03-surfaces-and-leftovers` Task 6). |
| e | glTF `skin.skeleton` is missing when the source omitted it | **open** | `glb.rs` writes `skeleton` only when `Skin::skeleton` is `Some`. The FBX reader always leaves it `None` (`fbx_read.rs`, `Skin { …, skeleton: None }`), and the USD reader sets it only for a single root. |
| f | animation resampling has no key-count cap | **half fixed** | `normalize::resample` caps the grid at `MAX_RESAMPLE_INTERVALS` (100,000) per clip, and `fits_fps` says when a clip is past it. Nothing budgets channels × grid against `scene::MAX_ANIMATION_KEYS` (20M) before the keys are allocated. |

So this spec fixes b, e and f, and asks the builder to re-run the tests that prove a, c
and d and then mark them closed.

## b. Joint names across a chained animate

**Why.** A caller rigs a mesh with `joint_naming: unity` and gets Unity names. They then
animate that rig by chaining (`source_model: {type: generation, value: <rig id>}`). Tripo
and Meshy animate their own stored rig by its task id, so the clips come back with the
vendor's joint names. The animation then can't be played on the rig the caller already
has, and the manifest's `skin.joints` disagrees with the rig's. Animate rows take no
`joint_naming` param (`models/tripo.yaml`, `meshy.yaml` and `mock.yaml` animate rows), so
the caller can't fix this either.

**The mock hides the bug.** `mock/animate-3d` animates `source.bytes`, the rig as
litegen delivered it, already renamed, even when it chains by the rig's task id
(`providers/model3d/mock.rs`, `ANIMATE_MODEL`). Real vendors animate their own copy.

**Decisions (mine):**

1. When chained by task id, the mock animates the rig job's own stored mesh, with the
   vendor's names, as Tripo and Meshy do. When the source arrives inline (data URI), it
   still animates `source.bytes`. This makes the existing bug visible to tests first.
2. An **animation** inherits the joint naming its rig was delivered with.
   `resolve_source_model` already walks an animation chain up to its rig (`chain_from`,
   at most 8 steps). From that rig row's recorded plan (`metadata.delivery.joint_naming`)
   it records the scheme in `SourceInput.joint_naming: Option<String>` and in the
   lineage record (`"joint_naming"`), for any chained source. It's a fact about the
   source. `DeliveryPlan::for_request` decides:
   - the request names `joint_naming` → that, with `vendor` meaning none;
   - it names none and the model's task is `animate` → `extras.source.joint_naming`;
   - otherwise → none.

   Only animate inherits. A texture, remesh or segment of a rig, or a re-rig, can return
   an unskinned mesh or a new skeleton. Renaming those would only add the "litegen cannot
   read this rig's joints as a humanoid" note. Both the client's poll and the poller
   build the plan this way, so they deliver alike. The adapter's `extras.joint_naming`
   stays as the request sent it: a vendor never receives a naming the caller didn't ask
   for.
3. Only a rig chained from a litegen generation passes its naming on. A mesh sent by URL
   or base64 keeps whatever names the vendor returns.
4. A request that names `joint_naming` itself, `vendor` included, wins over the
   inherited one. No carried animate row takes the param today, so this only matters
   for operator rows.

## e. `skin.skeleton` in written glTF

**Why.** glTF 2.0 §5.28.2 makes `skin.skeleton` optional. When the scene has none (every
FBX source, and multi-root USD), litegen's GLB has none either. The scene knows the
joints' common root, so the writer can say it. The research ([`research-consumers.md`](../research/research-consumers.md)
Q8) found that **no consumer needs it**, and that **a wrong value is a glTF-Validator
error**. So the value is small and the risk is a bad index. The validator run and the
ancestor-rule check below are what make it safe to ship.

**Decision (mine):** when a skin's `skeleton` is `None`, `glb::write_scene` and
`write_gltf_scene` write the **closest common root** of its joints in the written node
graph. That is the deepest node that is an ancestor-or-self of every joint, and it may be
a joint. glTF §5.28.2 allows exactly this: "MUST be the closest common root of the joints
hierarchy or a direct or indirect parent node of the closest common root". A
`skeleton` the scene already holds is written unchanged. When the joints sit in different
root trees there is no common root, so none is written. The reader is unchanged: a read
scene still says what its file said, and only the writer fills the gap.

## f. A budget on fps resampling

**Why.** `resample` builds `channels × (intervals + 1)` keys before anything checks the
scene's caps. The grid is capped at 100,000 intervals per clip, but channels × grid
isn't. A clip of 3,000 channels, two keys each over 3,000 s, is about 6k keys and a few
hundred KB of GLB. Keyed at 30 fps (90,000 intervals, under the grid cap) it becomes
3,000 × 90,001 = 270,003,000 keys, about 3.2 GB of floats. Keys aren't the whole cost
either: a weights channel holds one value per morph target per key, up to 1,024
(`MAX_MORPH_TARGETS`), so even 20M keys can be about 80 GB. `validate` would refuse
these only after allocating them.

**Decision (mine):** a new pure function `normalize::rekeyed_size(scene, anim, fps) ->
Option<RekeyedSize>`, where `pub struct RekeyedSize { pub keys: usize, pub values: usize }`,
returns what the scene would hold after rekeying clip `anim`, or `None` past the grid
cap.

- `keys` is the clip's channel count × its new key count, plus every other clip's
  existing keys.
- `values` sums keys × stride: 3 for translation and scale, 4 for rotation, the target
  count for weights. CUBICSPLINE keys in the untouched clips count ×3, and the rekeyed
  clip becomes linear.

`fits_fps` requires `keys <= scene::MAX_ANIMATION_KEYS` (20,000,000) and `values <=
pose::MAX_RESAMPLED_VALUES` (32,000,000, the writers' resampling budget), checked in
that order. A clip with no range, or `fps == 0`, still fits, as today. `resample` checks
it first and returns `false` before allocating anything. Delivery's note names the
first limit that fails:

- `None` keeps today's "clip {name} was not rekeyed at {fps} fps: it is too long to key
  at that rate";
- keys: "clip {name} was not rekeyed at {fps} fps: it would hold {keys} keys, over the
  20000000-key limit";
- values: "clip {name} was not rekeyed at {fps} fps: it would hold {values} values, over
  the 32000000-value limit".

## Non-goals

- Renaming joints in a URL or base64 source to match a rig the caller holds elsewhere.
- Writing `skin.skeleton` into formats other than glTF. USD and FBX have their own
  root conventions, and they already work.
- Changing `MAX_RESAMPLE_INTERVALS` or `MAX_ANIMATION_KEYS`.
- The three load-sensitive timing tests (fbx `scale_animated_ancestors_are_found_once_per_chain`,
  `many_distinct_key_time_arrays_write_in_linear_time`, threemf
  `composites_are_mixed_once_and_base_indices_cannot_multiply_primitives`). They aren't
  on this list, and the overview lists them as adjacent work.

## What proves it is done

### Re-proving a, c and d

Run these, and record each pass in the ledger with its name: model3d_deliver
`options_that_cannot_be_applied_are_named_on_the_asset` and
`names_stay_unique_after_joint_naming`; `grep -rn joint_renames litegen-core/src` returns
nothing. Then mark a, c and d closed in the matrix page data (tell litegen-a3, which owns
`3d-matrix/build.mjs`) and in this set's overview.

### b: tests (RED first)

- `mock_animate_by_task_id_animates_the_rigs_own_mesh` (mock.rs unit): rig with
  `joint_naming: unity`, then animate chained by the rig's task id. The mock's animated
  GLB has `MOCK_JOINTS` names. Inline, it has the source's names.
- `an_animation_chained_from_a_named_rig_keeps_its_joint_names` (tests/animate_api.rs):
  rig `mock/rig-3d` with `joint_naming: unity`, then `mock/animate-3d` with
  `source_model` = the rig's id. Assert:
  - the animated GLB's skin joint names, read with `mesh::read_scene`, equal the rig
    GLB's, in order;
  - the manifest's `scene.skin.joints` and `scene.skin.humanoid` are equal on both
    assets;
  - the animate row's `metadata.delivery.joint_naming == "unity"`;
  - `metadata.source.joint_naming == "unity"`.

  RED before decision 2 (names are the mock's), GREEN after.
- `a_chain_of_animations_keeps_the_rigs_names`: rig (unity), then animate A, then animate
  B from A. B's joints equal the rig's.
- `a_rig_delivered_with_vendor_names_passes_nothing_on`: rig without `joint_naming`,
  then animate. The joints are the mock's, the plan's `joint_naming` is absent, and no
  naming note appears.
- `a_vrm_rig_named_by_the_vendor_stays_vrm`: `joint_naming: vrm`, which the mock applies
  itself, as Tripo's `spec` does. The animation's names are VRM and equal the rig's.
- `the_poller_delivers_the_inherited_naming`: the same chain with the client never
  polling. The poller delivers from the recorded plan, and the names equal the rig's.
- `an_explicit_vendor_naming_is_not_overridden`: an operator animate row that takes
  `joint_naming`, asked `vendor` on a unity rig. The plan has no `joint_naming`, and the
  joints are the mock's.
- `only_animation_inherits_the_naming`: `mock/texture-3d` chained from the unity rig.
  The plan has no `joint_naming`, and no naming note is written.
- Negative control: drop the `.or(source.joint_naming)` fallback in a scratch copy.
  `an_animation_chained_from_a_named_rig_keeps_its_joint_names` fails with the mock's
  names. Restore by rewriting the file, never with `mv`.

### e: tests (RED first)

- `glb_writes_the_closest_common_root_when_the_scene_names_no_skeleton` (glb.rs):
  - the `skinned` fixture with `skeleton = None` → the written JSON's
    `skins[0].skeleton` is the index of the fixture's root joint;
  - a variant whose joints hang under a non-joint `Armature` node and start at two
    sibling joints → the `Armature` index;
  - joints in two separate root trees → no `skeleton` key.
- `a_skeleton_the_scene_names_is_written_unchanged`: `Some(k)` is written as `k`, even
  when `k` is an ancestor of the common root.
- `fbx_skins_reach_glb_with_a_skeleton` (tests.rs cross-format): read
  `testdata/fbx/skinned_small_assimp_7500.fbx`, write GLB, and assert `skeleton` is
  present and is an ancestor-or-self of every joint in the written graph.
- Khronos glTF-Validator on the written files: 0 errors, 0 warnings. Run it in the
  harness, and add a `matrix_tests` check that the field, when present, satisfies the
  ancestor rule.
- Harness: the `skinned` fixture's GLB/glTF cases still pass three.js at 0.00 %
  (three.js ignores `skeleton`; this proves nothing else changed).
  `node scripts/mesh-verify/read-gltf.mjs` on the FBX→GLB output lists the same joints.
- Negative control: write the common root's parent's parent, still legal, and the
  ancestor-rule check still passes. Write a *child* of the common root, illegal, and
  the ancestor-rule check fails. This shows the check can tell the two apart.

### f: tests (RED first)

- `rekeyed_size_counts_every_channel` (normalize.rs):
  - 3,000 nodes, each with a 2-key translation channel from 0 to 3,000 s, at 30 fps
    (90,000 intervals, under the grid cap): `keys == 270_003_000` and
    `values == 810_009_000`;
  - with a second, untouched clip of 500 translation keys: `keys == 270_003_500`;
  - the same scene from 0 to 3,600 s (108,000 intervals): `None`.
- `rekeying_past_the_key_budget_is_declined_without_allocating`: on the 3,000-channel
  scene, `fits_fps(scene, 0, 30)` is `false`, `resample(scene, 0, 30)` returns `false`,
  and the scene equals a clone taken before the call. Both checks come before any key is
  built, so the test is deterministic and needs no timing bound. Timing bounds are what
  make the three load-sensitive mesh tests flaky.
- `rekeying_past_the_value_budget_is_declined`: a 3-vertex mesh with 1,024 morph
  targets and one weights channel from 0 to 2,000 s, at 30 fps. That's 60,001 keys
  (under 20M) and 61,441,024 values (over 32M), so `fits_fps` is `false`, `resample` is
  `false`, and the scene is unchanged.
- `rekeying_within_the_budget_still_rekeys`: 100 channels over 10 s at 30 fps → `true`,
  and every channel has 301 keys.
- `the_notes_name_the_budget_that_failed` (model3d_deliver):
  - the 3,000-channel clip with `fps: 30` gives `fatal` `None`, the GLB delivered, and
    a note containing "over the 20000000-key limit";
  - the 1,024-target clip gives "over the 32000000-value limit";
  - the existing `u32::MAX` fps case keeps its "too long to key at that rate" wording.
- Negative controls, one per term: remove the key term from `fits_fps` and the key test
  fails at its `fits_fps` assertion. Remove the value term and the value test fails.

### Gate

- `cargo test --lib -- mesh::normalize mesh::glb mesh::tests` while iterating, then the
  full gate in the overview, including `tests/animate_api.rs`, `tests/model3d_chain.rs`
  and `tests/model3d_api.rs`.
- The harness's fixtures set, GLB and glTF rows: no change from the last run's verdicts.
- Docs:
  - `docs/superpowers/3d-format-conversion.md` §3 (glTF writer: `skeleton`);
  - the 3D doc's joint-naming paragraph (§3B), which gains one sentence: "a chained
    animation keeps its rig's names";
  - this set's overview, where a–f are marked closed.

## Left to the builder

- Where in `resolve_source_model` the rig row's plan is read (it's already loaded as
  `chain_from`).
- The common-root algorithm: walk each joint's ancestors in the written graph and keep
  the deepest node they share.
