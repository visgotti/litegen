//! 3D model (mesh) generation providers.
//!
//! Every provider here is async submit + poll, and returns downloaded bytes so
//! the caller can re-host them on litegen storage (see `Model3dProvider`).
//!
//! # The canonical 3D vocabulary, per vendor
//!
//! A request speaks litegen's vocabulary (`types::Model3dGenerationRequest`,
//! handed to adapters as `providers::Model3dExtras`); each adapter maps it onto
//! its vendor's own field names. A cell is filled only where the vendor's
//! published schema has the field and a test in that adapter asserts the
//! outbound name — `docs/superpowers/param-mapping-matrix.md` names the test.
//! A blank cell means the model's catalog entry does not declare the param, so
//! the validator refuses it (strict) or drops it (lax) before an adapter sees it.
//!
//! | litegen            | `fal/hyper3d-rodin`             | `fal/trellis` | `fal/hunyuan3d-v2` | `mock/*`        |
//! |--------------------|---------------------------------|---------------|--------------------|-----------------|
//! | `prompt`           | `prompt` (≤ 1024 chars)         | —             | —                  | tints the mesh  |
//! | `init` image(s)    | `input_image_urls[]` (≤ 5)      | `image_url`   | `input_image_url`  | accepted        |
//! | `output_formats`   | `geometry_file_format` (1/job)  | — (glb only)  | — (glb only)       | glb, obj, stl   |
//! | `seed`             | `seed` (0..=65535)              | `seed`        | `seed`             | —               |
//! | `pbr`              | `material` = `PBR` \| `Shaded`  | —             | —                  | accepted        |
//! | `texture`          | —                               | —             | `textured_mesh`    | accepted        |
//! | `target_polycount`, `topology`, `symmetry`, `rig` | — | —          | —                  | accepted        |
//!
//! Operations take a mesh instead of a prompt (3D doc §3A, §3B):
//!
//! | litegen          | `tripo/rig`                            | `fal/meshy-rig`                     | `mock/rig-3d`    |
//! |------------------|----------------------------------------|-------------------------------------|------------------|
//! | `source_model`   | `input`: its task id, or a `file_token` | `model_url` (glb as a `data:` URI)  | any glb          |
//! | `skeleton`       | `rig_type` (`humanoid` → `biped`)      | —                                   | `humanoid` only  |
//! | `joint_naming`   | `spec` (humanoid rigs only)            | —                                   | `vrm` renames    |
//! | `height_m`       | —                                      | `height_meters`                     | scales the rig   |
//! | `output_formats` | `out_format` (glb \| fbx, 1/job)       | glb, plus Meshy's fbx when promised | glb              |
//! | preflight        | `rig-check` (free) → 422 `not_riggable` | —                                   | flat → `not_riggable` |
//!
//! | litegen          | `tripo/animate`                        | `meshy/rig`, `meshy/animate`        | `mock/animate-3d` |
//! |------------------|----------------------------------------|-------------------------------------|-------------------|
//! | `source_model`   | `input` = its rig's task id only       | rig: `model_url` (`data:`) or `input_task_id`; animate: `rig_task_id` only | rigged glb |
//! | `animations`     | `animations` (preset ids, ≤ 5)          | `action_ids` (library ids, ≤ 10, no repeats) | one clip per entry |
//! | `in_place`       | `animate_in_place` when every clip asks | — (litegen holds the clip)          | natively when every clip asks |
//! | `fps`            | — (litegen keys the clips)              | `post_process: change_fps` (24/25/30/60, fbx) | keys at `fps` |
//! | `skeleton`, `height_m` | —                                | rig: `animation_type`, `height_meters` | —           |
//!
//! `output_formats` is the one param that is a promise rather than a hint:
//! adapters ask the vendor for ONE container, and litegen derives the rest
//! locally (`proxy::model3d_convert`). A vendor's own format field matters for
//! what litegen cannot derive, and for `prefers_vendor_native` containers
//! (fbx, usdz) whose vendor exports carry more than a conversion would.
//!
//! # Adding a vendor
//!
//! A new `Model3dProvider` is a module here, an arm in `proxy::registry`'s
//! `build_model3d_provider` (and `MODEL3D_PROVIDERS`), and its catalog rows in
//! `models/{vendor}.yaml` — each with a `docs_url` and `# @see` lines for the
//! schema and price it was read from. Its tests should prove, against a local
//! mock of the vendor: the outbound field for every canonical param it maps,
//! the auth header, every terminal state including the vendor's failure
//! envelopes, and that a transient poll failure stays retryable.
//!
//! A vendor's FREE check that decides whether the billed call may run
//! (Tripo's `rig-check`) is `Model3dProvider::preflight`: the handler runs it
//! before the submit and answers a refusal 422, billing nothing. A flow with a
//! second BILLED call after the first finishes (Meshy's preview before
//! refine) reports the boundary from `poll_status` as `next_stage` and
//! implements `start_stage`; `proxy::model3d_stages` claims the stage on the
//! row so only one observer starts it, and persists the context
//! `start_stage` returns.

pub mod cube;
pub(crate) mod download;
pub mod fal;
pub mod glb;
pub mod humanoid;
pub mod meshy;
pub mod mock;
pub mod tripo;
