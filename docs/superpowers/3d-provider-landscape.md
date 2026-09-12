# 3D provider landscape & output unification

Researched 2026-09-12. Every vendor row below was read from the vendor's own API
reference and then adversarially re-checked by a second pass; claims that
survived only one pass are marked **unverified**.

Companion to `param-mapping-matrix.md`. The design decision at the end is the
input to §14 of `specs/2026-08-19-3d-model-generation-design.md`.

---

## 1. Do 3D providers output the same files?

No. There are three structurally different shapes, and the difference is not
cosmetic — it changes what a caller can be promised.

| Shape | Vendors | What the caller gets |
|---|---|---|
| **Bundle** — many formats from one job | Meshy | `model_urls` carries glb + obj + fbx + stl + usdz (+3mf on request) from a single generation. `target_formats` narrows it; omitted means "all except 3mf". PBR maps arrive *separately* in `texture_urls`. |
| **One-of** — pick a single container per job | Hyper3D Rodin, Tencent Hunyuan3D | Rodin's `geometry_file_format` is a scalar enum `glb\|usdz\|fbx\|obj\|stl`, default `glb`. One job, one container. |
| **GLB-only** — no choice at all | Stability (SF3D, SPAR3D), most fal/Replicate/Runware slugs, Sloyd's API, Segmind | The response body *is* the GLB. |
| **Convert-as-a-second-job** | Tripo | Generation emits GLB. GLTF/FBX/USDZ/OBJ/STL/3MF require a separate, separately-billed convert task. `quad: true` force-overrides the output to FBX. |

**The one universal:** every vendor with a public API can emit **GLB**. That is
the only format a cross-provider guarantee can actually be built on.

Two second-order differences that bite:

- **Texture delivery.** Stability embeds textures in the GLB. Meshy hands back
  loose `base_color` / `metallic` / `normal` / `roughness` PNGs alongside it.
  Same `format: "glb"`, materially different deliverable.
- **Sync vs async.** Stability's 3D endpoints are *synchronous* — the mesh comes
  back in the response body, not via a task poll. That does not fit
  `Model3dProvider`'s submit+poll trait (`providers/mod.rs`); its adapter has to
  fabricate an already-terminal handle.

## 2. Are we unifying it today?

**The envelope, yes. The format, no.**

What already works:

- `output_format` is a canonical param (`capabilities/schema.rs:223`), validated
  against each model's declared `enum_values` by `validate_model3d`
  (`api/middleware/validator.rs:457`), with the repo's standard strict-vs-lax
  split: unsupported errors in strict mode, drops in lax.
- `capabilities.output_formats` is **derived** from that same param spec
  (`api/handlers/mod.rs:1140`), so a model cannot advertise a format its
  validator would reject. Single source of truth — this is good and should be
  kept.
- Storage is already format-agnostic *by design*: `model3d_asset_key`
  (`proxy/storage.rs:263`) takes the extension from the caller specifically so
  that `ImageStore::store`'s content-type sniffing cannot save a `.glb` as
  `.png`. An `.fbx` or `.usdz` survives rehost intact.

What does not:

1. **Nothing checks delivered against requested.** No code path compares
   `Model3dAsset.format` to the requested `output_format`. `mock/all-params-3d`
   advertises `[glb, obj, fbx, usdz]` (`models/mock.yaml:333`) while
   `MockModel3dProvider::generate` takes `_extras` and ignores it — so a request
   for `usdz` validates, bills, completes, and returns a `.glb`.
2. **`format` is a free-form `String`** (`types/mod.rs:186`) with the legal
   values living only in a doc comment. No allowlist, no case-folding, and it is
   interpolated straight into the storage key — a vendor-supplied `format`
   containing `/` or `..` becomes part of the S3 path.
3. **`mesh()` returns the *first* Mesh asset** (`types/mod.rs:222`), and that
   backs the `result_url` column. Under any multi-format design, whichever file
   the adapter pushed first silently becomes the canonical URL.
4. **Rehost de-dupes on stem, not `(stem, format)`** (`proxy/router.rs:1261`), so
   a second mesh lands at `model_1.fbx` — contradicting the function's own doc
   comment promising `model.<ext>`.
5. **Multi-file formats are structurally broken.** The mesh stem is hardcoded
   `"model"`. An `.obj`+`.mtl` pair or `.gltf`+`.bin`+textures gets renamed, and
   the reference *inside* the mesh file stops resolving. Only self-contained
   containers (glb, fbx?, usdz, stl, ply) survive. FBX is the open question —
   FBX textures are normally *external*; embedding is optional.
6. **No real vendor is wired.** `models/mock.yaml` is the only catalog file
   declaring `media_type: model3d`, and `MockModel3dProvider` is the only
   production `Model3dProvider` impl. Every "the adapter will…" statement is
   unexercised.

## 3. How unified are the params?

Better than the formats. There is a genuine canonical vocabulary —
`output_format`, `texture`, `pbr`, `rig`, `symmetry`, `topology`,
`target_polycount`, plus base `seed` / `negative_prompt` — and it maps close to
1:1 onto the leading vendors:

| litegen | Meshy | Rodin | Tripo | Stability |
|---|---|---|---|---|
| `output_format` | `target_formats` (array!) | `geometry_file_format` | convert-task `format` | — (GLB only) |
| `topology` | `topology` | — | `quad` (bool; forces FBX) | `remesh` (`none\|quad\|triangle`) |
| `target_polycount` | `target_polycount` (100–300k) | — | `face_limit` | `vertex_count` (−1..20000) |
| `pbr` | `enable_pbr` | `material` (`PBR\|Shaded\|All\|Hybrid\|None`) | `pbr` | — |
| `symmetry` | `symmetry_mode` (**deprecated**) | — | — | — |
| — | `texture_resolution` (2k/4k/8k) | `tier` | `texture_size` | `texture_resolution` (512/1024/2048) |

Our `target_polycount` bounds (100–300,000, `models/mock.yaml:310`) are exactly
Meshy's, which tells you what the schema was designed against.

Gaps worth naming:

- **`symmetry` is betting on a deprecated param.** Only Meshy has it, and Meshy
  deprecated it.
- **No canonical texture-resolution param**, despite three of four vendors
  exposing one. It would land in each model's `extra_allowlist` as a raw
  passthrough — i.e. un-unified by default.
- **`topology` is not one concept.** Meshy's `topology` picks quad-vs-triangle
  output; Tripo's `quad` *also silently changes the container to FBX*. Mapping
  them to the same canonical name hides a format side-effect.
- **Aggregators defeat the vocabulary.** fal's 3D slugs name the format param
  four different ways (`output_format`, `geometry_file_format`, `export_format`,
  `target_formats`) and several have none at all. A fal 3D adapter needs a
  per-slug `match` — the exact shape `replicate.rs:111-128` exists as a
  cautionary tale about.

## 4. Recommended design: `output_formats` as a declared, enforced set

**Adopt.** Not conversion — declaration plus enforcement.

- Replace the scalar `output_format` with **`output_formats: Vec<String>`**,
  backed by a new `ParamSpec::StringArray { enum_values, max_items, default }`.
  Keep `output_format` in `KNOWN_PARAMS` for one release; the rename must be
  atomic with the yaml edits or the loader rejects the catalog at boot, and
  deployments point at their own models dir.
- Each model declares only what it can **natively** emit, with `max_items`
  expressing the shape: Rodin `max_items: 1`, Meshy 5–6, Stability
  `enum_values: [glb]`. `glb` is mandatory in every 3D model's set.
- **Validate before spending.** An unsatisfiable request fails at validation with
  `param_enum_mismatch` / `param_too_many`, not after the vendor is billed.
- **Guard delivered ⊇ requested in all three completion observers** —
  `poller.rs:271-285`, `get_3d_status` (`handlers/mod.rs:795-812`) and
  `model3d_response_from_row` (`handlers/mod.rs:999-1012`) each independently
  enforce "completed ⇒ has mesh" today. Adding the subset check to fewer than
  all three means the row-answered path reports `completed` what the other two
  call `failed`.
- Supporting fixes, same commit: `mesh()` prefers glb; rehost keys on
  `(stem, format)`; a canonical `format → content_type` table
  (`glb → model/gltf-binary`, `stl → model/stl`, `3mf → model/3mf`,
  `usdz → model/vnd.usdz+zip`; **fbx and ply have no IANA registration** →
  `application/octet-stream`); extension sanitisation in `model3d_asset_key`;
  and the mock honouring `extras.output_formats`, since it is the only subject
  that can test the guard.
- `metadata` is **replaced, never merged** (`db/trait_def.rs:69-80`), and
  `persist_model3d_result` writes `json!({ "assets": ... })` over every sibling
  key. Anything stored there needs a deliberate merge-preserving write, tested.
  Related: nothing in the tree ever *writes* `stage_context` — `poller.rs:208`
  reads it and every producer sets `None`.

### Rejected: server-side format conversion

Considered and declined, for three independent reasons:

1. **FBX writing is legally encumbered.** The Autodesk FBX SDK license is the
   only complete path; open writers are incomplete. (Sourced from Unity's
   republication of the ALSA — grounds to avoid, not legal advice.)
2. **OBJ and glTF are multi-file**, and our rehost scheme renames siblings, which
   breaks the internal references (see gap 5 above).
3. **Cross-vendor convert leaks the asset.** Meshy's convert endpoint does accept
   a `model_url` at 1 credit, and Tripo's accepts a public model link — so it is
   *technically* available. It would mean shipping a Stability-generated mesh to
   Meshy on a publicly-fetchable URL, billed to litegen's Meshy credits, to
   satisfy a promise the caller could have been told up front was unavailable.
   That is a product decision, not an engineering one.

USDZ specifically does not need a pipeline: `<model-viewer>` generates it
client-side for AR from the GLB (**unverified** — second-hand from
model-viewer's FAQ).

---

## 5. Vendors — where to get keys

### First-party, self-serve

| Vendor | Modes | Formats | Format selection | Auth | Notes |
|---|---|---|---|---|---|
| **Meshy** — docs.meshy.ai | text, image, texture, remesh, rig | glb, obj, fbx, stl, usdz, 3mf | `target_formats` array; omitted ⇒ all but 3mf | `Authorization: Bearer` | Best fit for a bundle contract. Separate PBR maps. Convert endpoint 1 credit. API may require Pro tier — marketing page and auth page disagree. **No documented refund on failure.** |
| **Tripo (VAST AI)** — developers.tripo3d.ai | text, image, multiview, texture, retopo, rig | GLB native; others via convert task | separate billed convert task (10–30 credits) | `Authorization: Bearer` | Webhook push. `quad` forces FBX. Base 5 credits, 10 if any of `quad`/`face_limit`/`texture_size`/… is non-default. Documents freeze-then-refund. Model id spelling self-contradicts across docs pages — settle with one live call. |
| **Hyper3D Rodin (Deemos)** — docs.hyper3d.ai | image, text | glb, usdz, fbx, obj, stl | `geometry_file_format`, scalar, default glb | Bearer | **One format per job.** API access is Business tier — ~$120/mo, $1,152/yr. A purchase order, not a signup. |
| **Stability AI** — platform.stability.ai | **image only** (SF3D, SPAR3D) | glb | none | `Authorization: Bearer sk-...` | **We already have this key.** Synchronous — GLB in the response body, no polling. 10 credits (SF3D) / 4 (SPAR3D), no charge on failure. Doesn't fit the submit+poll trait. |
| **Tencent Hunyuan3D** | text, image, multiview | GLB default; OBJ/FBX/STL/USDZ | export option | TencentCloud signature (not bearer) | Face count 3k–1.5M. JobId valid 24h. Vendor docs disagree on namespace/host (`hunyuan` v2023-09-01 vs `ai3d` v2025-05-13) — resolve first. |
| **Sloyd** — sloyd.ai/api | text, parametric | GLB, FBX, OBJ, USD, USDZ, BLEND, 3MF, STL, PLY | — | — | Widest export list found. **Unverified** — docs host failed DNS from the research sandbox. |
| **Alpha3D** | text, image | glb, + often fbx/obj/stl | downloads endpoint | — | **Unverified**; its own docs contradict whether a plain generation already exposes fbx/obj/stl. |
| **Kaedim** | image, sketch | obj, fbx, glb, gltf | — | — | **Sales-gated, not self-serve.** |
| **Polycam** — poly.cam/docs/api | scanning / photogrammetry, not generative | glb, obj, fbx, stl, dae, usdz, ply, las… | export-job endpoints for the less common ones | — | Different product category. |

### Aggregators — one key, many 3D models

| Platform | Auth | 3D catalogue | Notes |
|---|---|---|---|
| **fal.ai** | `Authorization: Key $FAL_KEY` (**not** Bearer) | Tripo, Rodin, Hunyuan3D, Trellis, Meshy | **We already have a fal key** (`fal.yaml` in the catalog). Cheapest path to a first real 3D adapter — but the format param is named four different ways across slugs and output fields differ per model (`model_mesh` vs `model_glb`). |
| **Replicate** | token | Hunyuan3D 3.1, Trellis, Tripo | **Already in our catalog.** |
| **WaveSpeedAI** | `Authorization: Bearer` | 33 3D endpoints — TRELLIS, Hunyuan3D v2/v2.1/v3/v3.1, Meshy v6/v7, Tripo, Rodin v2/v2.5, Hitem3D, SAM 3D, **ByteDance Seed3D 2.0** (not on fal) | Broadest single-key 3D catalogue found. Per-model pricing published. |
| **Runware** | — | trellis-2, meshy-6, rodin-gen-2, sam-3d-objects | First-class `taskType: "3dInference"`. GLB only. ~$0.0077/run billed by compute time. |
| **Segmind** | — | glb, obj, ply, stl | |
| **3D AI Studio** | `Authorization: Bearer` | resells Hunyuan/Tripo/Meshy | GLB + optional PBR, credit-based, no subscription. |

### No public 3D generation API — don't chase

Luma Genie, CSM.ai (Cube), Masterpiece X, Spline, Backflip AI, Google, OpenAI,
Meta (3D Gen / AssetGen). Consumer apps, research, or discontinued. Stability's
**SV3D** also has no API route despite the model existing.

Also a lead, not a vendor: Shutterstock Generative 3D (NVIDIA Edify) — NVIDIA
retired the Edify NIM preview 2025-06-06 and current status is unconfirmed.

---

## 6. Recommended order of acquisition

1. **Nothing** — build the `output_formats` param against the mock first; it is
   the only subject that can test the delivered-vs-requested guard.
2. **fal.ai** — key already in hand; unlocks Tripo/Rodin/Hunyuan/Trellis behind
   one adapter. Accept the per-slug `match`.
3. **Meshy** — the only vendor that makes a multi-format bundle contract natural.
4. **Stability** — key already in hand, but it needs the sync-adapter shim first.
5. **Tripo direct** — only if per-format convert billing is acceptable.
6. **Rodin** — last; it carries a $1,152/yr floor.
