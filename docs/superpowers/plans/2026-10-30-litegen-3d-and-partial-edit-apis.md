# litegen: 3D operations and partial edits — what PixelGenie needs

- **From:** PixelGenie (`pixelgenie-v2`), 2026-10-30.
- **For:** the session building litegen's 3D layer, and Joe.
- **litegen read at:** `origin/3d-models` @ `9f1349e`. That branch was force-pushed: it is now `main` @ `2dc6573` plus 100 commits.
  - The local clone's `deploy/litegen-visgotti-instance` is 14 commits ahead of its remote and predates the rebase.
  - Start from `origin/3d-models`.
- **Supersedes:** L5 (rigging and animation), L6 (region and mask edits), L11 (multiview) and L12 (3D settings) of [`2026-10-29-litegen-requirements.md`](2026-10-29-litegen-requirements.md). That doc read `e0676a3`, the branch before the rebase; §1 below says what it got wrong about the current tree. Its other requests (L1–L4, L7–L10, L13–L19) still stand.

## 0. The prompt

> Read pixelgenie-v2's `docs/handoffs/2026-10-30-litegen-3d-and-partial-edit-apis.md`, then its `docs/handoffs/2026-10-29-litegen-requirements.md`. Build both on `origin/3d-models`, P0 first across the two.
>
> - The 2026-10-30 doc replaces the older one's L5, L6, L11 and L12.
> - The older doc's code citations come from the stale `e0676a3` tree. Re-check each of its requests against `origin/3d-models` before building; the newer doc's §1 lists the ones already fixed.
> - The newer doc's §2 rules decide every API shape.
> - Each item ships with its mock, a conformance test, its rows in `docs/superpowers/param-mapping-matrix.md`, and OpenAPI plus both SDKs regenerated in the same change.
> - Re-check any vendor row marked **(unverified)** against the vendor's docs before building on it.

## 1. What litegen already has

| Surface | What it does today |
|---|---|
| `POST /v1/models3d/generations` | Text-, image- and multiview-to-3D. The mode is inferred from reference roles: none means text, `init` means image, `view-*` means multiview. Canonical params: `output_formats`, `texture`, `pbr`, `target_polycount`, `symmetry`, `topology`, `rig` and `seed`. Vendor-specific settings go through `extra_allowlist`. Strict mode refuses an unsupported param; lax mode drops it and names it in `X-Litegen-Dropped-Params`. |
| `GET /v1/models3d/{id}`, `GET /v1/generations/{id}` | The job's state, plus `assets: [{ kind: mesh \| texture \| preview, url, format, size_bytes, polycount, width, height }]`. Every requested container is delivered, or the job fails. |
| `POST /v1/models3d/cost` | A quote at the base price. Multipliers are not applied. |
| `PATCH /v1/generations/{id}` | Cancels the job, and fal is told to cancel too. |
| `POST /v1/utils/models3d/convert`, `GET /v1/utils/models3d/formats` | Converts between ten containers in every direction: glb, gltf, obj, stl, ply, fbx, usdz, usda, 3mf and dae. The scene model is shaped like glTF: a node hierarchy, PBR materials with textures, skins, morph targets and animations. Anything a target can't hold is named in `X-Litegen-Conversion-Losses`. The reader refuses Draco- and meshopt-compressed GLBs. |
| Catalog | `ModelCapabilities` reports `supports_{text,image,multiview}_to_3d`, `supports_{pbr,rig,texture}`, `output_formats` and `max_polycount`. `ModelSchema` carries `docs_url`, `params` and `ref_inputs`. |
| 3D vendors | fal only: `fal/hyper3d-rodin` (text, or 1–5 images), `fal/trellis` (one image) and `fal/hunyuan3d-v2` (one image). There is no first-party Meshy, Tripo, Stability or Rodin adapter. The mocks are `mock/mesh-3d`, `mock/all-params-3d`, `mock/fail-3d` and `mock/partial-3d`. |
| Image edits | A `mask` reference role exists only on OpenAI, Stability and `mock/inpainting-image`. The mask's bytes go through in each vendor's own convention. There is no `region` field. |
| Video | `first_frame` and `last_frame` roles. A video can't be used as an input in any form. |

**Corrections to the 2026-10-29 doc:**

- **Fixed on `3d-models`:**
  - The 3D row is written before the submit returns (L2, for 3D).
  - A 3D submit that timed out is never re-submitted.
  - A base64 reference reaches a URL-only vendor as a `data:` URI (L4). This works for vendors that accept data URIs, as fal does.
  - The GLB reader reads skins and animations. L5 said it rejected skinned meshes.
  - 3D asset URLs are absolute on S3-compatible stores.
- **Still open on `3d-models`:**
  - There is no `Idempotency-Key` (L1).
  - In local mode, 3D assets live in process memory (L3).
  - Price multipliers are not applied (L12, L14).
  - `is_available` is hard-coded to `true` (L14).
  - `stage_context` is written once, so no vendor flow with a second billed stage can work.
  - Only mock models declare `rig`.
  - Draco and meshopt GLBs are refused.

## 2. Rules for every addition

These rules keep each addition usable by any client, not just PixelGenie.

1. **An operation is a catalog model, and each modality has one job endpoint.** Rigging, animation, texturing, remeshing and segmentation are models whose `task` says what they do, for example `tripo/rig` or `meshy/animate`.
   - They are submitted to their modality's existing endpoint, which for 3D is `POST /v1/models3d/generations`. They are polled, priced, cancelled, re-hosted and billed exactly like a generation, because each one produces a new asset.
   - fal, Replicate and LiteLLM all expose vendor operations as models. Validation, pricing, routing and `docs_url` then stay per operation, with no new routes.
   - **New `ModelSchema.task`**, which also appears on `ModelInfo.capabilities.task`. `generate` is the default in every modality.

     | Modality | Tasks |
     |---|---|
     | 3D | `generate` \| `rig` \| `animate` \| `texture` \| `remesh` \| `segment` |
     | Image | `generate` (which still covers edits with `init` and `mask`) \| `multiview` \| `background_swap` |
     | Video | `generate` \| `extend` \| `retake` \| `video_edit` |

   - The loader allows exactly one task per model. The `*_to_3d` flags are allowed only on 3D `generate` models.
   - A 3D `generate` model still infers text, image or multiview from its references.
   - Every other 3D task requires `source_model` (§3A).
   - Every video task except `generate` requires `source_video` (§5).
   - Image tasks take their input as the `init` reference, as edits do today.
2. **Inputs chain by generation id.** A new reference type, `{ type: "generation", value: "<litegen id>" }`, works in `source_model` and, for stored images, in `reference_images`.
   - litegen loads the stored bytes, checking access as `GET /v1/generations/{id}` does.
   - When the source came from the same vendor account, litegen passes that vendor's own task id rather than uploading the file again. Tripo and Meshy chain this way, and some of their operations require it. Clients never see vendor task ids, so only litegen can make that call.
3. **glTF 2.0 is the canonical format, and litegen enforces its conventions on delivery.**
   - **Conventions:** +Y up, metres, right-handed, with the asset's front facing +Z and its right side at −X ([glTF 2.0 §3.4](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html)).
   - **Orientation:** each model declares its vendor's `native_orientation`, proven by a live test. When it differs, litegen adds a root rotation.
   - **Inspection:** litegen inspects every mesh it delivers with its own reader and reports what it found (§3D). It never reports the vendor's claims.
4. **Canonical vocabularies, with vendor values allowed.** There are four:
   - skeleton types;
   - VRM 1.0 humanoid bone names for the joint map;
   - a short list of animation presets, mapped per model;
   - view names tied to yaw angles.

   A vendor value outside these lists still passes through, labelled `vendor`.
5. **The catalog comes first, and every feature has a mock.**
   - Every new request field is a `KNOWN_PARAMS` entry with a `ParamSpec`, and appears as a `supports_*` flag in the catalog.
   - Strict mode refuses an unsupported field; lax mode drops it and reports it.
   - Every feature ships with a deterministic mock and a conformance test.
6. **Refuse before billing.** Before the billed submit, check what the vendor requires of its input, using litegen's own mesh reader or the vendor's free preflight: format, size, face count, whether the mesh is rigged, and whether it can be rigged. The error names the requirement that failed.

## 3. 3D additions

### 3A. `source_model` and references by generation id (P0)

```json
"source_model": { "type": "generation", "value": "litegen-3d-…" }
"source_model": { "type": "url",    "value": "https://…/hero.glb", "format": "glb" }
"source_model": { "type": "base64", "value": "<bytes>",            "format": "fbx" }
```

- **Request.**
  - `format` is required for `url` and `base64`, and uses the `Model3dFormat` vocabulary.
  - litegen converts the source to whatever container the vendor accepts. For example, Meshy takes only GLB, and Tencent's rigging takes only FBX or OBJ.
- **Catalog.** Operation models declare `params.source_model`:

  | Field | Meaning |
  |---|---|
  | `formats` | Containers the vendor accepts |
  | `max_bytes` | Largest file the vendor accepts |
  | `max_faces` | Most faces the vendor accepts |
  | `requires_skin` | The source must already be rigged |
  | `requires_texture` | The source must be textured |

  litegen checks all of these with its own reader before submitting.
- **Errors.**
  - `source_not_found`: the generation id isn't one the caller can read.
  - `source_format_unsupported`
  - `source_too_large`
  - `source_too_many_faces`
  - `source_not_rigged`
  - `source_not_textured`
- **Record on the row.** How the source reached the vendor: `vendor_task`, `upload`, `signed_url` or `data_uri`.
- **Storage must be durable.** Chaining by id reads stored sources, so a litegen restart between two steps must not lose them. Keep local-mode 3D assets on disk, not in process memory, for at least 7 days. This makes L3's 3D half P0.

### 3B. Rigging (P0)

```json
{ "model": "tripo/rig",
  "source_model": { "type": "generation", "value": "litegen-3d-…" },
  "skeleton": "humanoid",
  "joint_naming": "vrm",
  "height_m": 1.7 }
```

- **New canonical params.**
  - **`skeleton`:** `humanoid` | `quadruped` | `hexapod` | `octopod` | `avian` | `serpentine` | `aquatic` | `other`. These are Tripo's `rig_type` values, with its biped renamed `humanoid`. A model lists the ones it rigs in `enum_values`.
  - **`joint_naming`:** `vendor` (the default) | `vrm` | `mixamo` | `unity` | `unreal`.
    - Where the vendor can do it, the value maps onto the vendor's setting; Tripo's `spec` is one.
    - Otherwise litegen renames the joints itself, for the vendors whose names it can map (P2). Renaming nodes never changes geometry.
  - **`height_m`:** Meshy's `height_meters`.
- **Rigging in the generation call.** The existing `rig: true` stays, and so does `supports_rig`, which means "takes `rig`". A generation model that rigs in the same call may also take `skeleton`.
- **Preflight.** Run the vendor's free preflight before the billed call, for example Tripo's `rig-check`. If the vendor says no, refuse with 422 `not_riggable` and bill nothing.
- **Output.** A rigged mesh. Its manifest's `skin` is filled in (§3D).

### 3C. Animation (P0)

```json
{ "model": "meshy/animate",
  "source_model": { "type": "generation", "value": "litegen-3d-…" },
  "fps": 30,
  "animations": [
    { "name": "walk",   "preset": "walk",         "in_place": true, "loop": true },
    { "name": "attack", "preset": "attack_melee", "in_place": true },
    { "name": "cheer",  "prompt": "raises both fists and cheers", "duration_s": 3 }
  ] }
```

- **Request.**
  - `animations` is a new canonical param with a new `ParamSpec` kind, `animation_list`.
  - Each entry has a unique `name` (1–64 characters, `[A-Za-z0-9_.-]`) and exactly one of `preset` or `prompt`.
  - `prompt` is text-to-motion, and is accepted only where the model offers it.
  - `fps` reuses the existing canonical param. It maps to the vendor's own setting (for example Meshy's `change_fps`), or litegen resamples the keys itself (`mesh::pose::resample_times`).
- **Catalog.**

  ```yaml
  animations:
    kind: animation_list
    max_items: 5                     # Tripo retargets up to 5 per call; Meshy takes 1–10
    presets:                         # vendor ids, each optionally tagged with a canonical id
      - { id: "preset:walk",  canonical: walk,         loopable: true }
      - { id: "preset:slash", canonical: attack_melee }
    text_to_motion: { min_duration_s: 4, max_duration_s: 10 }   # absent = no prompt clips
    in_place: native                 # native | litegen: how in_place is honoured
  ```

- **Presets.**
  - The canonical presets are `idle`, `walk`, `run`, `jump`, `fall`, `attack_melee`, `attack_ranged`, `cast`, `hurt`, `death`, `crouch`, `climb` and `turn`.
  - A request's `preset` can be either a canonical id or one of that model's vendor ids.
- **Output.**
  - One file with the skin and one glTF animation per entry, **named exactly `name`, in request order**.
  - Containers listed in `output_formats` are derived from it as today. Only glb and gltf are sure to keep every clip, because the converter's USD writer holds one clip and its FBX reader takes only the first animation stack. Losses are named in `X-Litegen-Conversion-Losses`.
- **In place.** `in_place: true` uses the vendor's own flag where one exists (Tripo's `animate_in_place`).
  - Otherwise litegen removes the horizontal (X and Z) translation of the root joint on its own scene model, keeping vertical movement. That is what Mixamo's "In Place" does.
  - The manifest says which of the two did it.
- **Loops.** `loop` is a hint only, recorded in the manifest. Nothing is resampled to force a loop.
- **Same-call clips.** A `rig` or `generate` model that returns clips in the same call takes `animations` too. `supports_animation` means "takes `animations`", whatever the task.

### 3D. Normalised delivery and the mesh manifest (P0)

Every `mesh` asset gains `sha256` and a `scene` object. litegen fills `scene` by reading the delivered file itself.

```json
"scene": {
  "units": "m", "up": "+y", "front": "+z", "oriented_by": "vendor",
  "bounds": { "min": [-0.31, 0.0, -0.18], "max": [0.31, 1.72, 0.20] },
  "vertex_count": 14201, "triangle_count": 24876, "uv_sets": 1, "vertex_colors": false,
  "materials": [{ "name": "body", "maps": ["base_color", "metallic_roughness", "normal"] }],
  "skin": {
    "skeleton": "humanoid", "joint_count": 24, "root": "Hips", "bind_pose": "t",
    "joints": ["Hips", "Spine", "…"],
    "humanoid": { "hips": "Hips", "spine": "Spine", "head": "Head", "leftUpperArm": "LeftArm", "…": "…" }
  },
  "animations": [
    { "name": "walk", "duration_s": 1.033, "in_place": true, "in_place_by": "vendor",
      "root_travel_m": [0.0, 0.01, 0.0], "loop_requested": true }
  ],
  "extensions_used": []
}
```

- **Orientation.**
  - `front` and `up` are always `+z` and `+y` (rule 3), and `oriented_by` says whether the vendor or litegen made them so.
  - A model whose `native_orientation` hasn't been proven reports `"front": "unverified"`. It never reports a guess.
- **Humanoid map.**
  - `humanoid` maps [VRM 1.0 humanoid bone names](https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_vrm-1.0/humanoid.md) to the file's joint names.
  - litegen holds one table per naming scheme it knows: Mixamo (`mixamorig:*`), Tripo, Meshy, UE5 Manny and Unity.
  - A rig no table matches gets `humanoid: null`, never a guess.
- **Bind pose.** `bind_pose` is `t`, `a` or `unknown`, read from the upper-arm angle in the bind pose.
- **No extension is needed to read a delivered file.** Every delivered glTF has an empty `extensionsRequired`.
  - Ask each vendor for an uncompressed file, or decode Draco and meshopt before re-hosting.
  - Transcode WebP and KTX2 textures to PNG, because glTF core allows only PNG and JPEG. fal's TRELLIS.2 sample, for instance, requires `EXT_texture_webp`. litegen's reader accepts that extension, but PixelGenie's renderer decodes only PNG and JPEG.
- **The vendor's file is kept.** Whenever litegen changes anything (a rotation, a decode, a transcode or a rename), it also delivers the vendor's own file as an `original` asset, so nothing is lost by normalising.
- **Joint names are unique.** glTF doesn't promise unique node names, so litegen adds a numeric suffix to any duplicate on delivery. The `humanoid` map can then name joints safely.
- **Lineage.** An operation's row records its source's generation id and `sha256` in `metadata.source`, so a chain can be audited and replayed.
- **Clip facts are measured.** `in_place`, `root_travel_m` and `duration_s` come from the keyframes, never from a vendor flag. glTF itself defines no loop, frame-rate or in-place behaviour, so these exist only as metadata.
- **Opt-in placement (P2).** The request may set `ground: true` (lowest point at y = 0), `center: true` (bounds centred on x and z) and `height_m`.
  - litegen applies them as one root-node transform, so skins and clips are unchanged.
  - PixelGenie frames models itself and won't send them.

### 3E. Multiview images and multiview-to-3D (P1)

- **Multiview images.** An image model with `task: "multiview"` on `POST /v1/images/generations` takes:
  - an `init` image;
  - `views: ["front", "left", "back", "right"]`, a string-array param whose `enum_values` are the views that model makes.

  It returns one `data[]` entry per view. `ImageResult` gains `view` and `yaw_deg`. This needs responses that return every image (L13).
- **Yaw convention, used everywhere.**
  - `yaw_deg` is the camera's position around +Y, measured from +Z (the subject's front) towards +X.
  - glTF puts the asset's right side at −X, so `front` = 0, `left` = 90, `back` = 180 and `right` = 270.
  - A view is named for the side of the subject the camera sees. The diagonals are `front-left` (45), `back-left` (135), `back-right` (225) and `front-right` (315).
- **Multiview-to-3D.** Reference roles are `view-<name>` from that same list. litegen maps each vendor's view names onto it.
  - "Left" is ambiguous at every vendor: the subject's left or the camera's.
  - So each mapping is proven with an asymmetric fixture, for example a character holding a sword in its right hand.
- **Real vendors.** Wire at least one real vendor for each half. Section 3H lists candidates.

### 3F. Texture, remesh, segment (P2)

| Task | Request | Output |
|---|---|---|
| `texture` | `source_model`, `prompt` and/or a `style` reference image, `pbr`, `texture_resolution` (new int enum), and `keep_uvs` | Textured mesh; `materials[].maps` reports the maps |
| `remesh` | `source_model`, `target_polycount`, `topology` | Retopologised mesh; `triangle_count` reports the result |
| `segment` | `source_model` | One glTF node per part, named with the vendor's label or `part_<n>`; the manifest gains `parts: [{ name, node, triangle_count, bounds }]` |

### 3G. Infrastructure these need

- **Stages after submit (P0).** `stage_context` must become updatable from a poll result.
  - Both observers of a job, the client's poll and the poller, persist it.
  - An atomic claim guards the start of each billed stage, so two observers can't both start it.
  - Needed for:
    - rig-check followed by rig;
    - an upload followed by its task;
    - Meshy's preview followed by refine.
- **`Idempotency-Key` on every billed POST (P0).** This is L1, and chained operations make it urgent: a rig retried after a crash bills again.
- **Durable 3D storage (P0).** See §3A.
- **Prices that match the bill (P1).**
  - Apply `variable_pricing`, which covers Rodin HighPack ×3, textured Hunyuan ×3, and per-operation credits.
  - Make the quote, the reservation and the charge change together.
- **A truthful `is_available` (P1).** Set it to `false` when the caller's org has no credentials for the provider.

### 3H. Vendors to wire

From a scan of vendor docs on 2026-10-30. Check each row before building on it.

| Need | Start with | Then |
|---|---|---|
| Rig a humanoid | **Tripo** `POST /v3/animations/rig`, after its free `rig-check`: `rig_type`, and `spec` = tripo, mixamo, actorcore, unity, unreal or vrm. It is the only vendor that documents its joint naming, and it chains by task id. | **Meshy** `/openapi/v1/rigging`: `height_meters`; needs a textured humanoid facing +Z. **fal** `fal-ai/meshy/rigging`, which works with the existing fal key. |
| Rig an animal | Tripo's `rig_type`: quadruped, hexapod, octopod, avian, serpentine or aquatic | Anything World `/rig` |
| Preset clips | Tripo `/v3/animations/retarget`: up to 5 per call, `preset:idle`, `walk`, `run`, `jump`, `slash` and others, and `animate_in_place` (default false) | Meshy `/openapi/v1/animations`: up to 10 `action_ids`, one GLB, `post_process.fps` of 24, 25, 30 or 60 |
| Text-to-motion | Meshy text-to-motion | Uthana (4–10 s; fps chosen at download); `fal-ai/hunyuan-motion` (returns FBX) |
| Multiview images | Tripo image-to-multiview: front, left, back and right | Meshy `generate_multi_view` |
| Multiview-to-3D | `tripo3d/h3.1/multiview-to-3d` on fal, or Tripo multiview-to-model with `inputs` keyed by view; `fal-ai/hunyuan-3d/v3.1/pro/image-to-3d`, which takes `back_`, `left_`, `right_`, `top_`, `bottom_`, `left_front_` and `right_front_image_url` | Meshy multi-image (1–4 unlabelled images, the first is the front); Tencent `MultiViewImages`; Rodin (1–5 unlabelled images) |
| Texture | Meshy `/openapi/v1/retexture`: `text_style_prompt`, `image_style_url` or `multiview_image_urls`; `enable_original_uv`; `enable_pbr` | Tripo `/v3/models/texture`, whose `part_names` repaints only the named parts; Tencent texture edit (FBX in) |
| Remesh | Meshy `/openapi/v1/remesh`: `topology`, `target_polycount` | Tripo `/v3/mesh/decimate`: `face_limit`, `quad`, `part_names`; Tencent `Submit3DSmartTopologyJob` |
| Segment | Tripo `/v3/mesh/segment`: later calls can address the parts by name | Meshy `/openapi/v1/print/split`; Tencent HY 3D Part; Sloyd `split-to-parts` |
| Better generation on the existing fal key | `fal-ai/trellis-2`, `fal-ai/hunyuan-3d/v3.1/pro/image-to-3d`, `meshy/v7.1`, `tripo3d/p2` | — |

**Facts that shape the adapters:**

- **Orientation.**
  - Tripo exports X-forward by default (`export_orientation` defaults to `+x`), so ask it for the glTF front.
  - fal's Hunyuan 3.1 Pro sample isn't Y-up: a +90° rotation about X fixes it.
  - The Meshy 7.1 and TRELLIS.2 samples are centred at roughly unit size, while Hunyuan's sits on y = 0.
  - These come from reading fal's example outputs.
- **Result URLs expire:**

  | Vendor | Kept for |
  |---|---|
  | Replicate, Kaedim | 1 hour |
  | Tripo, Tencent | About 24 hours |
  | Meshy | At most 3 days |
  | Rodin | 7 days |

  Keep re-hosting in the same tick the job completes.
- **Chaining.** Meshy chains through `input_task_id`, `rig_task_id` and `motion_task_id`. Tripo's `input` takes a task id. The others take URLs.
- **Joint names.** Only Tripo documents its naming. Record the real joint names from one billed rig run per vendor before freezing litegen's naming tables. No official Mixamo or UE5 Manny list was found **(unverified)**.
- **Retirements.**
  - Meshy's `latest` is now `meshy-7.1`.
  - `meshy-5` is retired, and Meshy's `lowpoly` retires on 2026-10-30.
  - fal marks its Seed3D endpoint deprecated.
- **No public API found:** Kinetix and Rokoko animation. CSM's docs didn't resolve **(unverified)**.

### 3I. Mocks

| Mock | Behaviour PixelGenie's tests check |
|---|---|
| `mock/rig-3d` (`task: rig`) | Returns a skinned box humanoid with joints `hips`, `torso`, `head`, `arm_l`, `arm_r`, `leg_l` and `leg_r`. Its `humanoid` map is `hips`→`hips`, `spine`→`torso`, `head`→`head`, `leftUpperArm`→`arm_l`, `rightUpperArm`→`arm_r`, `leftUpperLeg`→`leg_l` and `rightUpperLeg`→`leg_r`. `joint_naming: "vrm"` renames the joints to those VRM names. |
| `mock/animate-3d` (`task: animate`) | Returns the source plus one 1-second LINEAR clip for each requested `name`, in order. With `in_place: true` the root has no horizontal travel; with `false` it travels 1 m along +Z. It honours `fps`, and refuses an unrigged source with `source_not_rigged`. |
| `mock/turned-3d` (`task: generate`) | Emits a mesh facing +X and declares `native_orientation.front: "+x"`. The delivered file faces +Z, with `oriented_by: "litegen"`. |
| `mock/multiview-image` (`task: multiview`) | Returns one image for each requested view, tinted by its yaw and labelled with `view` and `yaw_deg`. |
| `mock/texture-3d`, `mock/remesh-3d`, `mock/segment-3d` | P2, alongside §3F. |

Keep `mock/mesh-3d`, `mock/all-params-3d`, `mock/fail-3d` and `mock/partial-3d` as they are.

### 3J. Done when

- A base64 still goes through this chain:
  1. `fal/trellis`, or any image-to-3D model;
  2. a `rig` model, given the first job's generation id;
  3. an `animate` model with an in-place `walk` and `idle`.

  The result is one GLB that `@gltf-transform/core` reads with two animations named `walk` and `idle`. Its manifest has a `humanoid` map, both clips report `in_place: true` with root travel under 1 cm, and the front faces +Z.
- Re-sending any step with the same `Idempotency-Key` returns the same generation id and bills nothing.
- Restarting litegen between two steps doesn't break the chain.

## 4. Partial image edits ("sub-rect")

### Verdict

**Masks, boxes, expanded canvases and per-box prompts become first-class litegen fields.**
- They are native vendor features.
- Every vendor spells them differently: three mask conventions, and four box formats.
- Turning those into one form is exactly what a gateway is for.

**Editing a rectangle on a model with no mask or box stays in the app.**
- The app crops, generates and pastes back, as Photoshop and Krita do.
- How much context to crop, how to resample and how to blend are each app's own choices.
- PixelGenie has to composite after pixelizing anyway.

### Evidence

From a scan of vendor docs on 2026-10-30.

| Feature | Native in | How each spells it |
|---|---|---|
| Mask inpainting | OpenAI GPT Image 2.5 and 2.6, Stability, BFL FLUX.1 Fill and Erase, Ideogram 4.5, Recraft, Bria | Transparent = edit (OpenAI). White = edit (Stability, BFL). Black = edit (Ideogram). |
| Box edit without a mask | Ideogram 4.5 `context_window`; BFL FLUX 3 Image `src_bbox` and `tgt_bbox`; Alibaba `wan2.7-image` `bbox_list` | Ideogram: `y_min,x_min,y_max,x_max` in source pixels, ends exclusive. FLUX 3: `[top,left,bottom,right]` on a 0–1000 scale. Alibaba: `[x1,y1,x2,y2]` in pixels. |
| Expand (outpaint) | BFL Expand and outpainting, Stability, Recraft V3, Bria, Kling, Ideogram 3 reframe, Adobe Firefly | Pixels per side. Or a canvas plus an offset. Or ratios per side (Kling). Or centred only (Ideogram). |
| A prompt per box, in one generation | BFL FLUX 3 Image, Ideogram 4.x JSON prompts, Bria Fibo 2 `structured_prompt`, Recraft V3 `text_layout` (text only) | Boxes guide the layout in all four; none clips to them. |
| Masks named in text | Stability search-and-replace and search-and-recolor; Bria's erase, replace and add by text | — |
| Pixels outside the edit kept exactly | Bria ("pixel-perfect preservation"); Ideogram 4.5 ("copied exactly" for pixels the edit "did not meaningfully change") | The others promise "similar" pixels, or nothing. |
| Marks drawn on the input | OpenAI's `markup` field (GPT Image 2.5 and 2.6), whose purpose the docs don't state | Seedream shows marks only in its marketing. |

**Retired since the 2026-10-29 doc:**
- Vertex `imagen-3.0-capability-001`, which had semantic masks and outpaint.
- Nova Canvas's `maskPrompt`.

### API (P0 unless marked)

```json
{ "model": "ideogram/ideogram-4-5-precise-edit",
  "prompt": "make the visor glow cyan",
  "reference_images": [
    { "type": "base64", "value": "<init png>", "role": "init" },
    { "type": "base64", "value": "<mask png>", "role": "mask" } ],
  "region": { "x": 384, "y": 256, "w": 128, "h": 64 },
  "mask_grow_px": 0 }
```

- **One mask convention.**
  - The `mask` is an opaque PNG the size of `init`. White (128 or more) means edit; black means keep.
  - A mask with any pixel whose alpha is below 255 is refused with 400 `mask_ambiguous`. OpenAI reads transparency the other way round, so a translucent mask can't be read safely.
  - litegen converts the mask for each vendor, and reports `mask_convention: "white_edits"` on every model that takes one.
- **`region: { x, y, w, h }`** is in init-image pixels, with a top-left origin and exclusive ends.
  - It uses named keys, never arrays, because the vendors disagree on axis order.
  - A box-native vendor receives the box in its own form. A mask-only vendor receives the box drawn as a mask, which is exact and leaves no choices to make.
  - When a request carries both `region` and `mask`, the edit area is where they overlap.
  - **Vendor minimums.** Ideogram's window must be at least 256 px on each side, with an aspect ratio between 1:6 and 6:1.
    - When a region is smaller, litegen enlarges the window to that minimum and sends the exact region as the mask.
    - A vendor that can't take both is refused with `region_too_small`.
- **`mask_grow_px`** (default 0).
  - It maps to the vendors that dilate a mask (Stability's `grow_mask`, BFL Erase's `dilate_pixels`). Send them an explicit 0, because their defaults are 5 and 10.
  - For every other vendor, litegen dilates the mask itself.
- **Errors, all before billing:** `init_required`, `mask_size_mismatch`, `mask_ambiguous`, `region_out_of_bounds` and `region_too_small`.
- **Catalog:**

  | Field | Meaning |
  |---|---|
  | `supports_inpainting` | Takes a `mask` |
  | `supports_region_edit` | Takes `region` |
  | `region_input` | `box` or `mask`: how the vendor receives a region |
  | `preserves_unmasked` | `exact`, `similar` or `unspecified`, from the vendor's own words and checked by a live test |
  | `mask_convention` | `white_edits` once litegen converts masks |
- **P2:**
  - **`expand: { left, top, right, bottom }`**, in pixels, each 0 or more.
    - The result is the whole canvas, with the source placed at (`left`, `top`).
    - A vendor that only centres takes only symmetric expansions, and anything else is refused with `expand_unsupported`.
    - Kling's ratios are converted, and the delivered canvas size is reported.
    - Catalog: `supports_expand`.
  - **`layout: [{ box: { x, y, w, h }, prompt, kind: "object" | "text", text? }]`**, for text-to-image.
    - Boxes are fractions of the canvas, from 0 to 1, because the output size may not be known yet. A `region` uses pixels because the init image's size is known.
    - litegen compiles the layout into each vendor's prompt JSON. The boxes guide the layout everywhere; none clips to them.
    - Catalog: `supports_layout`.
  - **`mask_prompt`**: a mask named in text, for Stability search-and-replace and Bria's by-text edits. Catalog: `supports_mask_prompt`.
  - **`preserve_unmasked: true`** (opt-in): litegen pastes the init's pixels back outside the edit area, with a hard edge or `feather_px`.
    - Untouched pixels then come back byte-identical from any vendor.
    - It is allowed only when the output will be the same size as the init. Otherwise it is refused before billing.
  - **Background swap** as its own `task`, for Recraft, Ideogram, Bria and Stability, beside L8's background removal.
- **Not canonical: marks drawn on the image.** Only OpenAI has a field for them, and it doesn't document what the field does. The app burns marks into the init image, or passes OpenAI's `markup` through `extra`.

### Vendors to start with

- **The first real `region` model:** Ideogram 4.5 Precise Edit. It keeps untouched pixels exactly, and litegen already has an Ideogram adapter.
- **Then:**
  - BFL FLUX 3 Image (the BFL adapter exists);
  - Alibaba `wan2.7-image` (no Alibaba adapter yet).
- **Mask models to add:** BFL FLUX.1 Fill (white = edit), Bria `gen_fill` (keeps pixels exactly) and Ideogram 4.5's mask. Keep OpenAI and Stability.

### Mocks

| Mock | Behaviour |
|---|---|
| `mock/inpainting-image` (existing id) | Fills exactly the edit area (where the mask and the region overlap) with `#ffcd75`, and leaves every other pixel byte-for-byte identical. Honours `mask_grow_px`. |
| `mock/region-image` | Declares `region_input: "box"` and a 256 px minimum window, and fills the same way. It proves the window enlargement. |
| `mock/expand-image` | P2. |

### Done when

- One white-edits mask confines the change to the masked area on both OpenAI and Stability, each converted to its own convention.
- `region` works on Ideogram 4.5, and on a mask-only model as a drawn mask.
- A translucent mask is refused before anything is billed.

## 5. Partial video edits: time ranges, keyframes and regions

### Verdict

- **First-class:** timed keyframes, extending a clip, regenerating a time window ("retake") and whole-clip edits. Each is native in several vendors. They become `task` models plus one new reference field.
- **Left out: masked video edits.** The four live vendors use four incompatible kinds of mask, and no flagship video editor takes a mask at all. Reach them through `extra` until the vendors converge.

### Evidence

From a scan of vendor docs on 2026-10-30.

| Feature | Native in | Notes |
|---|---|---|
| Timed keyframes | BFL FLUX 3 Video: `[seconds, image]` pairs, 1–10. Luma `ray-3.2`: `keyframes` plus `keyframe_indexes` at 24 fps, up to 64. Runway `aleph2`: `seconds` or `at` (0–1), up to 5, video-to-video only. PixVerse: `multi_transition` durations. | Veo 3.1, Gemini Omni, LTX, MiniMax, Seedance and Wan 2.7 take a first and last frame only. Pika's pikaframes was deprecated on 2026-10-29. |
| Extend | LTX, Veo 3.1 on Agent Platform, Gemini Omni, Luma `ray-3.2`, PixVerse v6, BFL FLUX 3 Video, Seedance 2.x, Wan 2.7. Kling only for its v1.x generations. | Luma and Kling take only one of their own earlier generation ids. LTX, BFL, Seedance and Wan take only an uploaded video. |
| Retake a time window | LTX `/v2/retake`: `start_time` plus `duration`, and `mode` = replace video, audio or both; the window is at least 2 s. Runway `aleph2`: `range{start_seconds, end_seconds}` in whole seconds. | — |
| Whole-clip edit | Runway `aleph2`, Luma `ray-3.2` `video_edit`, Gemini Omni, BFL video edit, Kling O1 and O3, Wan 2.7 videoedit, Seedance 2.0, xAI Grok | Only Runway takes a time range. None takes a mask. |
| Masked video | Wan VACE: a mask video, or a mask image plus a frame number. fal LTX-2.3 inpaint: a mask video. Kling 1.6 multi-elements: click points **(unverified)**. PixVerse swap: the vendor segments the frame itself. | Four kinds of mask that don't convert into each other. |

**Gone:**
- OpenAI's Sora and Videos API, shut down on 2026-09-24.
- Veo 2's masked insert and remove on Vertex.

### API

- **Timed keyframes (P1).**
  - A new `ReferenceImage.time_s` gives seconds from the start of the clip, on references with role `keyframe`.
  - `first_frame` and `last_frame` stay, as names for t = 0 and t = the clip's duration.
  - litegen converts seconds into each vendor's form: Luma's frame numbers at 24 fps, PixVerse's durations between frames and Runway's fractions.
  - Catalog: `supports_keyframes`, `max_keyframes` and `keyframe_timing: "any" | "even"`.
  - PixelGenie uses keyframes to pin key poses at fixed times: an attack's wind-up and hit, or a walk's contact poses at 0 and at half the cycle.
- **`source_video` (P2).** It takes `{ type: "generation" | "url" | "base64", value }`, like `source_model`.
  - Luma and Kling accept only their own earlier generations. So for them, litegen accepts only sources made on that vendor account, and refuses any other with `source_not_chainable`.
- **Extend (P2).** A `task: "extend"` model takes:
  - `source_video`;
  - `direction: "forward" | "backward"`;
  - `duration_seconds`, the length to add;
  - `prompt`.

  The catalog says whether the vendor returns the whole clip or only the new part: `extend_output: "full" | "added"`. The job reports `segments: [{ start_s, end_s, origin: "source" | "generated" }]`. litegen doesn't splice video itself.
- **Retake (P2).** A `task: "retake"` model takes:
  - `source_video`;
  - `time_range: { start_s, end_s }`;
  - `replace: "video" | "audio" | "both"`;
  - `prompt`.

  A window shorter than the vendor's minimum (LTX's 2 s), or one off Runway's whole-second grid, is refused before billing.
- **Whole-clip edit (P2).** A `task: "video_edit"` model takes `source_video`, `prompt` and `reference_images`, plus `time_range` where the vendor takes one (Runway). Each vendor's own strength setting stays in `extra`.
- **Every finished video job** reports `{ duration_s, fps, width, height, has_audio }`, as L10 asks.

**For PixelGenie:** timed keyframes are P1, and the rest is P2. PixelGenie repairs single frames as image edits, because every frame is pixelized anyway.

### Mocks

| Mock | Behaviour |
|---|---|
| `mock/keyframe-video` (existing id) | Gains `keyframe` with `time_s`. The frame at each keyframe's time equals that keyframe's image. |
| `mock/extend-video`, `mock/retake-video` | P2. Frames outside the generated window are identical to the source's. |

## 6. What PixelGenie does itself

Don't build these for PixelGenie:
- rendering facings and animation frames from a GLB, and mapping joints and materials to parts;
- framing a model in a view, which is why it never sends `ground`, `center` or `height_m`;
- the crop-and-composite fallback for models that take neither a mask nor a box (§4), and compositing every edit back with locked pixels kept;
- pixelizing, palette snapping and extracting video frames.

## 7. The catalog fields PixelGenie reads

PixelGenie maps catalog evidence to its own capabilities in one table, so these names are the contract. Rows marked **new** need litegen work.

| PixelGenie capability | Catalog evidence |
|---|---|
| `model3d.image`, `.text`, `.multiview` | `supports_image_to_3d`, `supports_text_to_3d`, `supports_multiview_to_3d` |
| `model3d.rig` | `supports_rig` (rigs in the generation call), or `task: "rig"` (**new**) |
| `model3d.animate` | `supports_animation` (**new**: takes `animations`), plus `params.animations.presets` and `in_place` |
| `model3d.texture` | `supports_texture` (in the generation call), or `task: "texture"` (**new**) |
| `image.multiview` (**new**) | `task: "multiview"`, plus `params.views.enum_values` |
| `image.mask` | `supports_inpainting`, plus `mask_convention: "white_edits"` (**new**). Until that field appears, PixelGenie keeps its own table for each vendor. |
| `image.region` | `supports_region_edit` (**new**), plus `region_input` |
| Preferring exact edits | `preserves_unmasked` (**new**) |
| `video.keyframes` (**new**) | `supports_keyframes` and `max_keyframes` |
| Delivery facts | `Model3dAsset.scene` and `sha256` (**new**) |
