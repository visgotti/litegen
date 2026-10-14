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

pub mod cube;
pub mod fal;
pub mod glb;
pub mod mock;
