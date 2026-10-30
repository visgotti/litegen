# What PixelGenie needs from litegen

- **From:** PixelGenie (`pixelgenie-v2`), 2026-10-29.
- **For:** whoever builds litegen.
- **litegen surveyed at:** `e0676a3` on `deploy/litegen-visgotti-instance`, the same tree as `origin/3d-models` was in the local clone at the time.
- **Superseded in part (2026-10-30):** `origin/3d-models` had already been force-pushed to `9f1349e`, which is `main` plus 100 commits, so this survey read a stale tree. [`2026-10-30-litegen-3d-and-partial-edit-apis.md`](2026-10-30-litegen-3d-and-partial-edit-apis.md) replaces L5, L6, L11 and L12, and its §1 lists what the current tree already fixes.
- **PixelGenie sources:** spec §10.4 in `docs/superpowers/specs/2026-10-29-agentic-pixel-art-studio-design.md`, and the plan in `docs/superpowers/plans/2026-10-29-pixelgenie-studio.md`.

## 1. PixelGenie in one minute

PixelGenie is an agentic studio for pixel-art game assets: sprites, props with states, animations, top-down characters with 4 or 8 facings, tilesets, environments and modular rigs. A person or an agent asks for an asset, and PixelGenie builds it as a pipeline of steps: source, then pixelize, then edits, then rules. Every step is cached and can be re-run from any state.

What that means for litegen:

- **Every image, video and 3D request goes through litegen.** PixelGenie holds no provider keys and calls no provider directly.
- **Exact pixels matter.** Models make high-resolution sources, and PixelGenie reduces them to the native grid (for example 16×16), snaps them to a locked palette and checks them with rules. So the details below are hard requirements, not polish:
  - lossless output;
  - true transparency;
  - pixels outside an edit area left untouched;
  - a still camera in sprite videos.
- **Every paid call is ledgered.** The ledger is keyed by a hash of the request:
  - a long job is recorded with litegen's job id the moment it is submitted;
  - after a crash it is resumed by polling `GET /v1/generations/{id}`;
  - results are downloaded at once into PixelGenie's content-addressed store.
- **Features are gated on capabilities read from litegen's catalog, never on model names.** A new litegen flag turns on a PixelGenie feature through one new row in one table, so the exact flag and field names in this document are the contract.
- **Tests run against litegen's mock models at $0.** Every new capability needs a deterministic mock.

## 2. The catalog fields PixelGenie reads

PixelGenie maps catalog evidence to its own capabilities in exactly one table. Rows marked **new** need litegen work.

| PixelGenie capability | Catalog evidence it reads |
|---|---|
| `image.text` | `supports_text_to_image` |
| `image.refs` | an image model whose `ref_inputs` total is above 0 |
| `image.edit` | `supports_image_to_image` |
| `image.mask` | `supports_inpainting` and a `mask` ref role |
| `image.transparent` | `background` in `extra_allowlist` |
| `image.region` (**new**) | `supports_region_edit`, or `region` in `extra_allowlist` |
| `video.text` | `supports_text_to_video` |
| `video.firstFrame` | `supports_first_frame` or `supports_image_to_video` |
| `video.lastFrame` | `supports_last_frame` |
| `model3d.image` | `supports_image_to_3d` |
| `model3d.multiview` | `supports_multiview_to_3d` |
| `model3d.text` | `supports_text_to_3d` |
| `model3d.rig` | `supports_rig` |
| `model3d.texture` | `supports_texture` |
| `model3d.animate` (**new**) | `supports_animation` |

PixelGenie also reads, per model:
- `ref_inputs`: the role names, `min`, `max`, `required`, the default role, the total, and `provider_format` (`base64`, `multipart` or `url`);
- `extra_allowlist`;
- the supported sizes and the `max_images`;
- the video durations (minimum, maximum and default);
- the price.

Notes on today's catalog, from the survey:
- `max_images` in `GET /v1/models` is the reference-image limit.
- `max_duration_seconds` is always null, so PixelGenie reads durations from the single-model schema's `duration_seconds` parameter.
- `supports_rig`, `supports_texture` and `supports_multiview_to_3d` are true only on mock models.

## 3. What each pipeline step asks of litegen

| PixelGenie step | What it asks litegen for | Today |
|---|---|---|
| Sprite and prop stills: 4 to 8 candidates per subject, style references, transparent or flat `#ff00ff` background | Text-to-image with up to 14 references, one image per call, PNG, transparency | **Works, with gaps:**<ul><li>transparency only on OpenAI, through `extra.background`;</li><li>PNG only through `extra.output_format`, on some models;</li><li>in strict mode `seed` is rejected on Google, OpenAI, Luma, Kling, MiniMax and ByteDance.</li></ul> |
| Prop states, each referencing state 1 | Image with references | **Works** |
| Animation by video: the base still as the first frame (and as the last, for a loop), the model's default duration | Image-to-video with first and last frame, a still camera, no audio | **Partly:**<ul><li>first and last frames exist on Kling, Seedance, MiniMax Hailuo, Vidu, PixVerse and Luma;</li><li>camera and audio settings exist only in per-model `extra`;</li><li>**Google shut down all three Veo 3.1 preview models on the Gemini API on 2026-10-22**, and litegen still lists them (L14). Their results needed Google's key to download anyway (L3);</li><li>of the providers the local `litegen.yaml` enables, fal and Luma take frames only as URLs (L4), so with that config no real model can animate a still that PixelGenie sends.</li></ul> |
| Animation by sprite sheet or per-frame stills | Image with references, wide aspect ratios | **Works** |
| Region edits ("make the visor glow on frame 1", "fix frame 5's feet", "fill the arm hidden behind the shield"); PixelGenie tries a box, then a mask, then a crop | An edit limited to a box, an edit limited to a mask, image-to-image | **Partly:**<ul><li>masks only on OpenAI gpt-image and Stability SDXL, passed through raw;</li><li>no box targeting;</li><li>a mask without an init image silently becomes text-to-image (L6).</li></ul> |
| Selecting a part of a high-resolution source or reference | Promptable segmentation: a point, box or text in, a mask out | **No** (L7) |
| Clean cut-outs before pixelizing | Background removal | **No** (L8) |
| Top-down characters from 3D: still → 3D model → rig → walk, idle, run and attack clips; PixelGenie renders 4 or 8 facings from one model | Image-to-3D, rigging, animation | **Partly:**<ul><li>image-to-3D exists on fal (TRELLIS, Hunyuan3D v2, Rodin), but those take references only as public URLs (L4);</li><li>rigging and animation are a listed non-goal;</li><li>the GLB reader rejects skinned meshes (L5).</li></ul> |
| Top-down turnaround: one character in consistent front, side and back views | Multiview images; multiview-to-3D | **No** (L11) |
| Tilesets and environments | Image with references, wide sizes | **Works** |
| Spend caps: refuse a call that would pass a session's cap | A price before submitting | **Works:** `POST /v1/{images,videos,models3d}/cost` |
| Crash safety: never pay twice | Idempotent submits; a job row that exists | **No** (L1, L2) |
| Integration tests at $0 | Mocks that behave like the real models | **Partly:** mocks ignore references, masks and size (section 6) |

## 4. Requests, in priority order

- **P0:** PixelGenie is blocked without it, or can pay twice.
- **P1:** a planned PixelGenie feature needs it.
- **P2:** it improves cost, quality or ergonomics.

Every request ships with a deterministic mock (section 6).

| # | Request | Priority |
|---|---|---|
| L1 | Idempotent submits | P0 |
| L2 | A job row that exists when the submit returns | P0 |
| L3 | Results that litegen keeps and serves | P0 |
| L4 | Base64 references for URL-only vendors | P0 |
| L5 | 3D rigging and animation | P0 |
| L6 | Region and mask edits, with one mask convention | P0 |
| L7 | Promptable segmentation | P1 |
| L8 | Background removal | P1 |
| L9 | Output format and transparency as real fields | P1 |
| L10 | Video controls for sprites | P1 |
| L11 | Multiview images and multiview-to-3D | P1 |
| L12 | 3D settings sent to real vendors | P1 |
| L13 | Request-time checks: capabilities, `n`, model ids | P1 |
| L14 | A truthful catalog | P1 |
| L15 | Vendor errors that say what happened | P1 |
| L16 | Pixel-art vendors | P2 |
| L17 | Stored request metadata, and spend per tag | P2 |
| L18 | Cancelling at the vendor, and refunding quota | P2 |
| L19 | An SDK that is versioned, shows headers, and matches the server's timeouts | P2 |

## 5. Request details

File references are in the litegen repo at `e0676a3`.

### L1. Idempotent submits (P0)

- **Why.**
  - PixelGenie ledgers every paid call under a request hash.
  - If PixelGenie crashes after litegen accepts a job but before PixelGenie records the job id, its retry is a second billed job.
  - The same thing happens inside litegen today.
- **Today.**
  - There is no idempotency key anywhere in the server or the SDKs.
  - Images retry a 120 s timeout up to twice (`litegen-core/src/proxy/router.rs:177-208`), so one slow vendor call can start three billed jobs.
  - Several image adapters poll their vendor inside that call: BFL and Replicate for up to 4 minutes, Hunyuan for 3, and Kling, Leonardo, Luma and Runway as well. A slow job outlives the 120 s cap, so it is resubmitted and billed again.
  - Your own notes record the same problem for video: "A timed-out video submit is retried as a new billed generation" (`docs/superpowers/2026-09-11-model-drift-decisions.md`, section 6).
- **Need.**
  - An `Idempotency-Key` header on every POST that bills: images, videos and 3D. PixelGenie will send its ledger key, which is 64 hex characters.
  - The same key with the same body within 24 hours returns the original response and bills nothing:
    - the same generation id for a video or 3D job;
    - the same image for an image call, which means synchronous results are stored under the key.
  - The same key with a different body is a 409 `idempotency_key_reused`.
  - No automatic retry of a vendor call that timed out, unless that vendor is idempotent. Return a 504 that carries the vendor's job id when one exists.
- **Done when.**
  - Two POSTs with one key make one vendor call and one billing row.
  - A mock whose first attempt times out shows exactly one vendor call.

### L2. A job row that exists when the submit returns (P0)

- **Why.** PixelGenie polls `GET /v1/generations/{id}` immediately after submitting, and again after a restart.
- **Today.**
  - The row is inserted by a spawned task after the response has been sent, and insert errors are discarded (`litegen-core/src/api/handlers/mod.rs:440-456` for video, `:690-720` for 3D).
  - An immediate GET can return 404.
  - A failed insert leaves a billed job that nobody can poll or resume.
- **Need.**
  - Insert the `pending` row before responding: id, vendor job id, model, provider and cost estimate. If the insert fails, the request fails.
  - Store the request itself on the row (prompt, parameters, and reference hashes, not bytes), so a job can be audited and re-run.
- **Done when.**
  - Every id a submit returned answers 200, including on the very next request.
  - Killing litegen right after a submit loses nothing.

### L3. Results that litegen keeps and serves (P0)

- **Why.** PixelGenie downloads every result as soon as a job completes, and it holds no vendor keys.
- **Today.**
  - **Video.**
    - A video result is the vendor's own URL (`litegen-core/src/proxy/poller.rs:171`).
    - Veo's `video.uri` "must be downloaded with the API key (it is not a public CDN URL)" (`litegen-core/src/providers/video/google.rs:23-26`). Sora's works the same way (`providers/video/openai.rs:272-273`).
    - So PixelGenie could never fetch output from Veo 3.1, which its plan used as the default video model. Google has since shut those preview models down (L14), but every vendor whose URLs expire or need a key still needs re-hosting.
    - Hunyuan's URLs expire after 24 hours.
    - Inline video bytes are dropped (drift notes, section 6), which affects Sora.
  - **3D.** 3D files are re-hosted, but with the default storage they live in an in-process map and are lost on restart (`litegen-core/src/proxy/storage.rs:317-353`).
  - **Job memory.** `GET /v1/videos/{id}` forgets a job on its first finished read (`litegen-core/src/proxy/router.rs:609-615`), and forgets every job on a restart.
- **Need.**
  - Re-host every result in litegen storage when the job completes: images, video and every 3D file. Your 3D spec already makes this the rule (goal 3 of `docs/superpowers/specs/2026-08-19-3d-model-generation-design.md`); extend it to video, which closes the follow-up noted in `google.rs`.
  - `GET /v1/generations/{id}/assets`, listing every file as `{ kind, format, url, size_bytes, sha256, width?, height?, duration_s?, fps? }`.
  - `GET /v1/generations/{id}/assets/{n}`, streaming the bytes to anyone holding a litegen key.
  - Keep files for at least 7 days, on disk or in S3, through restarts.
  - Make `GET /v1/videos/{id}` read from the database, as the 3D route already does (`handlers/mod.rs:751-866`), and never delete on read.
- **Done when.**
  - A video downloads with only a litegen key, even after the vendor's own URL has expired.
  - A 3D model downloads after a litegen restart.
  - Reading a finished video twice gives the same answer.

### L4. Base64 references for URL-only vendors (P0)

- **Why.**
  - PixelGenie's 3D path depends on vendors that take references only as URLs: all of fal 3D, plus Luma, Runway, Vidu, Replicate images and fal video.
  - PixelGenie runs on a laptop with no public host.
- **Today.**
  - A base64 reference sent to a URL-form model is stored under `tmp/refs/<uuid>` by storage that is hard-wired to `LocalStorage` (`litegen-core/src/main.rs:121-124`). The vendor receives an unusable `local://` URL.
  - The file is also deleted when the request returns (`litegen-core/src/proxy/materializer.rs:69-76`, `:158-174`). So even with real storage, an asynchronous vendor that fetches it later would find nothing.
  - Leonardo and PixVerse already upload references to their vendors' own storage.
- **Need.**
  - Accept base64 for every model.
  - For a URL-form vendor, litegen either:
    - serves the bytes at an unguessable HTTPS URL that expires within the hour; or
    - uploads them to the vendor's own storage (fal has one).
  - Keep each published reference until the job that uses it has finished.
  - Then `provider_format` becomes litegen's internal business, and PixelGenie can drop its `publicBaseUrl` setting.
- **Done when.** `fal/trellis` accepts a base64 image from a client that has no public URL.

### L5. 3D rigging and animation (P0)

- **Why.**
  - A top-down character with 4 or 8 facings comes from one 3D model. PixelGenie renders each facing at a yaw angle, so the facings agree by construction.
  - It renders the walk, idle, run and attack frames from the model's animation clips, and labels parts (head, arm, leg) from the skin's joints.
  - Without rigging and animation, a generated model can be shown only in its bind pose. Today PixelGenie needs a rigged GLB uploaded by hand.
- **Today.**
  - Rigging and animation are listed in the "Non-goals" of `docs/superpowers/specs/2026-08-19-3d-model-generation-design.md`.
  - `rig` is validated, but only mocks declare it and no adapter sends it (`litegen-core/src/capabilities/schema.rs:305-312`).
  - The GLB reader refuses matrix accessors, so it cannot read any skinned GLB (`litegen-core/src/mesh/glb.rs:644-649`).
- **Need.**
  - **Reverse that non-goal.**
  - **Rig.**
    - `rig: true` on image-to-3D and text-to-3D for vendors that rig in the same call.
    - A rigging operation for an existing mesh: a 3D request whose `source_mesh` is `{ generation_id }` or `{ type: "base64" | "url", value, format: "glb" }`.
    - Catalog: `supports_rig`, and `rig_skeletons: ["humanoid", "quadruped"]`.
  - **Animate.**
    - A 3D request with a rigged `source_mesh` and `animations: [{ name, motion?, prompt?, in_place: true, loop: true, seconds? }]`.
    - `motion` is a vendor preset id; `prompt` is text-to-motion where the vendor offers it.
    - The output is one GLB containing the skin and one glTF animation per entry, named exactly `name`. Per-clip files are optional extras.
    - Catalog: `supports_animation`, `animation_presets: [{ id, name, tags, loopable }]` and `supports_text_to_motion`.
  - **Skeleton.**
    - Report the vendor's skeleton name and joint names in the asset metadata.
    - Any standard naming works, because PixelGenie maps joints to parts by name: it strips `_l`, `_r`, `.L` and `.R`, and maps `upperarm` to `arm`, `thigh` to `leg` and `spine` to `body`.
  - **Pose.** `pose: "t" | "a"` on image-to-3D, so the mesh rigs cleanly. Rodin's `TAPose` already exists as a pass-through.
  - **GLB handling.** Skins and animations must pass through the GLB reader and the local conversion untouched. At minimum, skip conversion for skinned meshes, and never fail a rigged result for having a skin.
  - **Vendors to start with:** see section 7.
- **Done when.** A base64 still goes through TRELLIS (or any image-to-3D model), then rigging, then `walk` and `idle` clips, and downloads as one GLB that `@gltf-transform/core` reads with two animations named `walk` and `idle`.

### L6. Region and mask edits, with one mask convention (P0)

- **Why.**
  - Most edits are partial. PixelGenie scales the 16×16 cell up onto the model's canvas and sends the target as a box or a mask.
  - It then pixelizes the result back and composites only the target pixels; locked pixels never change.
  - It needs the vendor to change only the target area.
- **Today.**
  - Only two families take a mask, and litegen passes the mask bytes through as they are, so a client has to know each vendor's convention:

    | Family | Mask convention | Where |
    |---|---|---|
    | OpenAI gpt-image | the edit area is transparent | `litegen-core/src/providers/image/openai.rs:234-258` |
    | Stability SDXL | the edit area is white (`mask_source=MASK_IMAGE_WHITE`, hard-coded) | `litegen-core/src/providers/image/stability.rs:351-394` |

  - An OpenAI mask sent without an init image is dropped, and the call quietly runs as text-to-image (`openai.rs:256-258`).
  - There is no box targeting, and the `inpainting` flag is not checked against requests.
  - The image mocks ignore masks and references (`litegen-core/src/providers/image/visual_mock.rs:14-60`).
- **Need.**
  - **One mask convention for every model that takes a mask.** The mask is a PNG the size of the init image, where white or opaque means edit and black or transparent means keep. litegen converts it for each vendor and declares `mask_convention: "white_edits"` on the model. Until that flag appears, PixelGenie keeps its own table for each vendor, and refuses any vendor it doesn't know.
  - **`region: { x, y, w, h }` as a real field, in init-image pixels.** For a vendor that takes only masks, litegen draws the box as a mask; where a vendor takes boxes, it sends the box directly. Catalog: `supports_region_edit`.
  - **A 400 `init_required`** for a mask or region sent without an init image.
  - **`preserves_unmasked: true | false`** per model. PixelGenie composites either way, but prefers vendors that keep unmasked pixels exactly.
  - **More models that take masks:** see section 7.
- **Done when.**
  - One white-edits mask confines the change to the masked area on both OpenAI and Stability.
  - `region` works on at least one real model.
  - The mock leaves every unmasked pixel byte-for-byte identical (section 6).

### L7. Promptable segmentation (P1)

- **Why.** These steps need a mask cut from a high-resolution image:
  - "select the visor" on a 1024-pixel source or a reference photo;
  - splitting a character into rig parts;
  - pulling props out of an environment painting.
- **Today.** Nothing. PixelGenie segments native pixel art itself, from its colour ramps, but that doesn't work on high-resolution images.
- **Need.**
  - A synchronous `POST /v1/images/segmentations`.
  - Request: `{ model, image (base64), prompts: [...] }`. Each prompt is one of:
    - `{ type: "point", x, y, label: 1 | 0 }`
    - `{ type: "box", x, y, w, h }`
    - `{ type: "text", text }`
  - Response: `{ masks: [{ png_base64 (white = selected), score, box }] }`.
  - Catalog: `supports_segmentation`, and `segmentation_prompts: ["point", "box", "text"]`.
  - **Vendors:** see section 7.

### L8. Background removal (P1)

- **Why.** PixelGenie asks for a flat `#ff00ff` background and keys it out, but models drift: soft edges, purple fringes and stray gradients. A matting model gives a clean alpha channel.
- **Today.** Nothing. Your provider audit lists it as unimplemented (`docs/superpowers/2026-08-16-provider-api-audit.md:1134-1138`).
- **Need.**
  - Background-removal models on the edit path. They take only an init image and return an RGBA PNG.
  - Catalog: `supports_background_removal`.
  - This is how LiteLLM exposes Stability's `remove-background` and Bedrock Nova's `BACKGROUND_REMOVAL`.
  - **Vendors:** see section 7.

### L9. Output format and transparency as real fields (P1)

- **Why.** JPEG artifacts break palette snapping, and true transparency saves keying out the background.
- **Today.**
  - `background` and `output_format` are not request fields. They work only through `extra`, on models that allow them:

    | Field | Models that accept it in `extra` |
    |---|---|
    | `background` | OpenAI |
    | `output_format` | OpenAI, Stability, BFL, fal FLUX, Replicate |

  - At the top level of a request, both keys are silently dropped.
  - `content_type` is whatever the adapter says, and it is often wrong:
    - OpenAI results are labelled PNG even when `extra.output_format` asks for another format (`litegen-core/src/providers/image/openai.rs:359`, `:369`);
    - Replicate FLUX returns WebP labelled as PNG;
    - Luma always returns JPEG.
- **Need.**
  - `output_format: "png" | "webp" | "jpeg"` and `background: "transparent" | "opaque"` as validated request fields.
  - Catalog: `output_formats`, `supports_transparent_background`, and `lossy_only: true` on models that can't return PNG.
  - `content_type` sniffed from the actual bytes.

### L10. Video controls for sprites (P1)

- **Why.** A sprite animation needs:
  - a still camera;
  - no audio, which costs extra;
  - a loop, where the last frame is the first;
  - a short duration;
  - frames that PixelGenie extracts with ffmpeg.
- **Today.**
  - First and last frames exist on several vendors.
  - Camera, audio and loop settings exist only in per-model `extra`:
    - Seedance `camera_fixed`
    - Kling `camera_control`
    - Luma `loop`
    - Veo always generates audio.
  - Durations are checked only against a minimum and maximum. Your drift notes list the vendors that take fixed values: Kling, Luma (5 or 9 s), PixVerse, MiniMax Hailuo (6 or 10 s), Veo (4, 6 or 8 s) and vidu2.0.
  - The finished job reports no duration, fps or size.
- **Need.**
  - `camera: "fixed" | "auto"`, `audio: false` and `loop: true` as request fields, mapped for each vendor.
  - Catalog: `supports_fixed_camera`, `supports_audio_toggle` and `supports_loop`.
  - A list of allowed values on `duration_seconds`.
  - A finished job reports `{ duration_s, fps, width, height, has_audio }`.
  - **Which vendors suit sprites:** see section 7.

### L11. Multiview images and multiview-to-3D (P1)

- **Why.**
  - Turnarounds: one character in consistent front, side and back views, for 4- and 8-facing sprites.
  - Better 3D models built from several views.
- **Today.**
  - Only `mock/all-params-3d` declares `view-*` roles.
  - Meshy and Tripo multiview adapters are designed but not started (`docs/superpowers/specs/2026-08-19-3d-model-generation-design.md:511-515`).
- **Need.**
  - A multiview image model: one image in, each requested view out as its own image, labelled with its view name. This needs responses that can return more than one image (see L13).
  - Multiview-to-3D on at least one real vendor.
  - Catalog: `supports_multiview` with the list of views, and `supports_multiview_to_3d`.
  - **Vendors:** see section 7.

### L12. 3D settings sent to real vendors (P1)

- **Why.** Low-poly meshes rasterize cleanly at sprite sizes, and PixelGenie wants a preview image and a face count for each model.
- **Today.**
  - `target_polycount`, `topology`, `symmetry`, `texture` and `pbr` are validated, but only mocks declare them.
  - fal returns no preview and no polycount, and only Rodin returns texture maps.
  - The Rodin and Hunyuan price multipliers are recorded but never applied (`litegen-core/src/providers/model3d/fal.rs:740`), so their cost is under-reported.
- **Need.**
  - Map those settings onto each vendor's own: TRELLIS `mesh_simplify`, Rodin `quality` and `tier`, Hunyuan `octree_resolution`.
  - Return a preview render and the face count.
  - Apply the price multipliers.

### L13. Request-time checks: capabilities, `n`, model ids (P1)

- **Today.**
  - **Capability flags.** The validator never reads them, so a request the model can't serve fails at the vendor instead (drift notes, section 6).
  - **`n`.** It is forwarded to vendors that bill for every image, but only the first image is returned (`litegen-core/src/proxy/router.rs:1296-1302`).
  - **Model ids.** Adapters silently swap an unknown model id for a default, so a misconfigured id is billed as a different model:
    - runway uses `gen4_image` / `gen4_turbo`
    - leonardo uses MOTION2 or Lightning XL
    - luma uses `ray-2`
    - fal uses `flux/dev`
    - google uses `gemini-3.1-flash-image` (`litegen-core/src/providers/image/google.rs:78-98`)
  - **Sizes.** OpenAI snaps an invalid size to its own grid without saying so (`litegen-core/src/providers/image/openai.rs:109-134`).
- **Need.**
  - Reject a capability the model lacks with a 400 `capability_unsupported`.
  - Either return all `n` images or reject `n > 1`. PixelGenie always sends `n: 1`.
  - Treat an unknown model id as an error, not a substitution.
  - Reject a size the model can't produce, rather than snapping it.

### L14. A truthful catalog (P1)

- **Today.**
  - `is_available` is always true, and the list ignores which providers have credentials (`litegen-core/src/api/handlers/mod.rs:1166`). This bites today: the local `litegen.yaml` enables only google, fal, luma and mock, so every model that takes a mask (OpenAI and Stability) reports itself available, then fails with `provider_not_configured`.
  - Models past their vendors' shutdown dates are still listed and served. Among them:
    - `google/veo-3.1-generate-preview`, `veo-3.1-fast-generate-preview` and `veo-3.1-lite-generate-preview` (2026-10-22 on the Gemini API; Google names `gemini-omni-1.1-flash` as the replacement). They are litegen's only Google video models (`models/google.yaml:124-178`).
    - `openai/gpt-image-1` (2026-10-23)
    - `openai/sora` and `sora-2-pro` (2026-09-24)
    - `bedrock/amazon.nova-canvas-v1:0` and `amazon.nova-reel-v1:1` (2026-09-30)
    - `kling/kling-v2`, `kling-v1-5`, `video-kling-v1-6` and `video-kling-v2-1` (2026-09-15)

    `litegen-core/tests/catalog_conformance.rs:89-104` records every date above except Veo's.
  - One recorded date is now wrong the other way: the test retires `google/gemini-2.5-flash-image` on 2026-10-02, but Google's deprecations page (updated 2026-10-22) now gives 2027-03-15.
  - Prices are flat per output (`litegen-core/src/proxy/router.rs:560-563`), so per-second video, per-megapixel FLUX and the 3D multipliers are all billed at the base price. Spend caps built on these prices run low. `apps/price-api` models those price types, but nothing calls it.
  - `max_images` is really the reference limit (`:1176`).
  - `max_duration_seconds` is always null (`:1177`).
  - Deprecations appear only in description text.
  - Model allowlists are stored but not enforced when generating.
- **Need.**
  - `is_available: false` when the caller has no credentials for that provider.
  - `max_reference_images`, keeping `max_images` as an alias.
  - Real durations.
  - `status: "active" | "deprecated" | "retired"`, plus `deprecated_at`, `retires_at` and `replaced_by`.
  - The three Veo 3.1 preview ids removed. For a Google video model, add `gemini-omni-1.1-flash` on the Gemini API, or the GA `veo-3.1-generate-001` and `veo-3.1-fast-generate-001` on Agent Platform (formerly Vertex AI), which stay until "Nov 17, 2026 or later".
  - A `catalog_version` that changes whenever any field changes.
  - Prices that match the vendor's bill (per second, per megapixel, multipliers), both in the catalog and from `/cost`.
  - PixelGenie currently hard-codes a list of ten retired models; it will read these fields instead.

### L15. Vendor errors that say what happened (P1)

- **Today.**
  - A vendor's 4xx at submit comes back as a 502 "All deployments failed" carrying only a message (`litegen-core/src/proxy/router.rs:208-215`, `:1444`).
  - `provider_error` is never filled in.
  - `code` is a string in validation errors and a number everywhere else, while the SDK types it as a string.
- **Need.**
  - A vendor's 4xx becomes a litegen 4xx `vendor_rejected`, with `provider_error: { status, code, message }`.
  - A vendor 5xx or timeout becomes 502 or 504, with `retryable`.
  - `code` is always a string.

### L16. Pixel-art vendors (P2)

- **Why.**
  - Two vendors draw native pixel art directly: PixelLab and Retro Diffusion. Their images need no reduction step, they take a forced palette, and they do things general models don't: 4- and 8-direction characters, animation from a still, and tilesets.
  - PixelGenie would use them as one more source, side by side with general models.
- **Today.**
  - Neither vendor has an adapter, and nothing in the catalog says a model draws native pixel art.
  - A few listed models have pixel-art or palette settings, but their output still needs reducing to the grid:
    - Stability's `style_preset`, whose values include `pixel-art` (`models/stability.yaml:97`);
    - Ideogram's `color_palette`, only through `extra`;
    - Recraft's `substyle` and `controls`, only through `extra`.
- **Need.**
  - Adapters for PixelLab v2 and Retro Diffusion. Both queue every job for polling, so they fit the async job path that video and 3D use.
  - Catalog: `native_pixel_art: true`, `pixel_size: { min, max }` (the output edge in native pixels), `supports_forced_palette`, and, where they apply, `directions: [4, 8]` and `max_frames`.
  - `palette: ["#rrggbb", ...]` as a request field. Map it to PixelLab's forced palette and Retro Diffusion's `input_palette`, and, as a hint only, to Ideogram's `color_palette` and Recraft's `controls`.
  - Output at native size. Never upscale it: leave Retro Diffusion's `upscale_output_factor` unset. PixelGenie scales its own previews.
  - Directions and frames returned as separate images, each labelled with its direction or frame index. A sprite sheet is fine if it comes with its grid: `frame_width`, `frame_height`, `columns` and `rows`. Either way this needs responses that return more than one image (L13).
  - Retro Diffusion's free `check_cost` dry run behind `/v1/images/cost`.
  - **Vendors:** see section 7.
- **Done when.**
  - A 16-colour `palette` sent to each vendor returns a native-size PNG that uses only those colours.
  - An 8-direction request returns 8 images, each labelled with its direction.

### L17. Stored request metadata, and spend per tag (P2)

- **Today.** `metadata` is accepted and then thrown away.
- **Need.**
  - Store `metadata` on the row: a string map of at most 1 KB.
  - Echo it in responses and webhooks.
  - Let `/v1/stats` and `/v1/logs` filter by a metadata key.
  - PixelGenie will send `{ project, asset, step, ledger_key }`, so spend can be split by project and asset.

### L18. Cancelling at the vendor, and refunding quota (P2)

- **Today.**
  - Cancel only marks the row. The vendor's job keeps running and billing.
  - Reserved quota is not refunded, and no webhook fires.
- **Need.**
  - Call the vendor's cancel where one exists, and report `vendor_cancelled: true | false`.
  - Refund quota when a job is cancelled or fails.

### L19. An SDK that is versioned, shows headers, and matches the server's timeouts (P2)

- **Today.**
  - `@litegen/sdk` 0.0.1 is unpublished and its `dist/` is stale, so PixelGenie copies `src/` at a known commit.
  - Timeouts don't match the server:
    - a request times out after 60 s, while the server may take 3 × 120 s;
    - job polling gives up after 5 minutes.
  - Response headers (`x-litegen-dropped-params`, the rate limits) are hidden.
- **Need.**
  - Publish the SDK with semantic versions and a changelog.
  - Expose response headers.
  - Default timeouts that cover the server's worst case.
  - Add an `idempotencyKey` option (L1) and `generations.assets()` (L3) as those land.

## 6. Mocks: every feature ships with one

PixelGenie's integration tests run against litegen's mock models, and its live tests are capped at $0.50 per run. So each new capability needs a mock that behaves like the real thing, deterministically and at $0.

| Mock | Behaviour PixelGenie's tests check |
|---|---|
| Inpainting, with a mask or a `region` | Fills exactly the masked or boxed pixels with one colour (for example `#ffcd75`) and leaves every other pixel byte-for-byte identical |
| Segmentation | A box prompt returns a mask equal to the box; a point prompt returns a 32×32 square around it |
| Background removal | Turns `#ff00ff` pixels transparent |
| Rigging | Returns a skinned box humanoid with joints `hips`, `torso`, `head`, `arm_l`, `arm_r`, `leg_l` and `leg_r` |
| Animation | Returns the source mesh with one 1-second clip per requested `name`, using linear keyframes |
| Multiview | Returns one image per requested view, each tinted by its yaw |
| Slow image | Times out on its first attempt, to prove L1 makes only one vendor call |
| Video with `camera`, `audio` and `loop` | Echoes the settings it received in the job's metadata |

PixelGenie already uses `mock/visual-image-gen`, `mock/inpainting-image`, `mock/keyframe-video`, `mock/mesh-3d` and `mock/fail-3d`. Please keep those ids.

## 7. Vendors to add

From a scan of vendor docs on 2026-10-29. Prices come from each vendor's own pricing page, except where marked:
- **(computed)** is arithmetic from a published rate;
- **(unverified)** could not be confirmed on an official page.

This market moves weekly, so check each row before building on it.

### Dates that affect the catalog now

- **Google, Gemini API.**
  - `veo-3.1-generate-preview`, `veo-3.1-fast-generate-preview` and `veo-3.1-lite-generate-preview` shut down on 2026-10-22. Google names `gemini-omni-1.1-flash` as the replacement ([deprecations](https://ai.google.dev/gemini-api/docs/deprecations), updated 2026-10-22).
  - The GA `veo-3.1-generate-001` and `veo-3.1-fast-generate-001` stay on Agent Platform (formerly Vertex AI) until "Nov 17, 2026 or later".
- **Google image models.**
  - `gemini-2.5-flash-image` now shuts down on 2027-03-15.
  - `gemini-3.1-flash-image` has no date, but Google points to `gemini-nano-banana-2.1` ($0.0336 per image at 1K), which litegen doesn't list.
- **OpenAI.** `gpt-image-1` shut down on 2026-10-23. litegen already lists `gpt-image-2.5-sunburst` and `gpt-image-2.5-flare`.
- **Don't add these:**
  - BytePlus retires `seedance-1-5-pro-251215` on 2026-11-11.
  - Meshy retires its `lowpoly` model on 2026-10-30.
- **APIs that moved; check the adapters:**
  - **Luma** moved to `agents.lumalabs.ai`, which lists only `ray-3.2`. litegen calls `https://api.lumalabs.ai/dream-machine/v1` (`litegen-core/src/providers/image/luma.rs:54`) and lists `ray-2` and `ray-flash-2`.
  - **Ideogram's** v1 paths are legacy; the current ones are under `/v2/image/`. litegen calls `/v1/ideogram-v3/generate` and `/v1/ideogram-v4/generate` (`litegen-core/src/providers/image/ideogram.rs:287`, `:295`).
  - **Kling's** image-to-video moved to `/image-to-video/kling-<version>`. litegen calls `/v1/videos/image2video` (`litegen-core/src/providers/video/kling.rs:128`).

### For L5: rigging and animation

| Vendor | Rigging | Animation | Price |
|---|---|---|---|
| **Tripo**, base URL `https://openapi.tripo3d.com/v3` ([rig](https://developers.tripo3d.com/en/docs/animations-rig.md), [retarget](https://developers.tripo3d.com/en/docs/animations-retarget)) | `POST /animations/rig-check` (free) reports `riggable`; then `POST /animations/rig`. Input: a task id, file token or URL (GLB, FBX, OBJ or STL, up to 150 MB). `rig_type`: biped, quadruped, hexapod, octopod, avian, serpentine, aquatic or others. `spec` for bipeds: tripo, mixamo, actorcore, unity, unreal or vrm. Set rig model `v3.0-20260909` explicitly for humanoids. | `POST /animations/retarget`: up to 5 `animations` per call, GLB or FBX, and **`animate_in_place`** (default false, which means root motion). Presets for v2.5 and v3.0 rigs: `preset:idle`, `walk`, `run`, `jump`, `slash`, `shoot`, `hurt`, `fall`, `turn`, `dive` and `climb`, plus quadruped, hexapod and octopod walks. v3 rigs have no death clip. | Rig 25 credits, each animation 10 credits; 1 credit = ¥0.07 ([pricing](https://developers.tripo3d.com/en/pricing)) |
| **Meshy** ([rigging](https://docs.meshy.ai/en/api/rigging), [animation](https://docs.meshy.ai/en/api/animation)) | `POST /openapi/v1/rigging`: `input_task_id` or `model_url` (GLB only, up to 400 MB), and `height_meters`. Textured humanoids only, facing +Z, at most 300k faces. Returns a rigged GLB and FBX, plus walking and running clips. | `POST /openapi/v1/animations`: `rig_task_id` plus `action_ids` (1–10) returns **one GLB with one clip per action, in request order**, named by library name. `post_process.change_fps`: 24, 25, 30 or 60. The free [library](https://docs.meshy.ai/api/animation-library) includes Idle 0, Attack 4, Dead 8, RunFast 16 and Regular_Jump 466. [Text-to-motion](https://docs.meshy.ai/en/api/text-to-motion) works for bipeds. | Rig 5 credits, 3 per action, text-to-motion 3 or 10; needs a paid plan, with no published dollar rate. On fal as [`fal-ai/meshy/rigging`](https://fal.ai/models/fal-ai/meshy/rigging): $0.20, or $0.32 with an animation |
| **Tencent HY 3D** ([`SubmitAutoRiggingJob`](https://www.tencentcloud.com/document/api/1284/79641)) | FBX or OBJ up to 60 MB, **T-pose required**, characters or animals. One job at a time by default; results are kept 24 h. | `MotionType` 1–48 bakes one preset into the rig call: 23–25 walk, 26 and 27 idle, 34 run, 38 and 46 jump in place, 44 fall forward, 3 charged attack | **(unverified)** |
| **Uthana**, GraphQL ([text-to-motion](https://uthana.com/docs/api/capabilities/text-to-motion), [retargeting](https://uthana.com/docs/api/capabilities/retargeting)) | Rigs an uploaded character when it needs one | `create_text_to_motion_job` with `text-to-motion-3.0`: 4–10 s clips, retargeted at download, FBX or GLB at 24, 30 or 60 fps | $0.10/s |

- **Where to start:** fal's `fal-ai/meshy/rigging` for humanoids, because fal is already enabled. Then Tripo for quadrupeds and for in-place clips.
- **In place:** PixelGenie renders a walk on the spot, so `in_place` matters. Only Tripo has a flag for it; Meshy doesn't say whether its clips are in place.
- **Looping:** no vendor documents whether its clips loop, so set `loopable` only where a vendor says so.
- **Clip names:** Meshy names clips after its library and Tripo after its presets, so litegen renames each clip to the requested `name`.
- **Re-hosting:** Tripo's result URLs expire after 5 minutes, so L3's re-hosting has to land first.
- **No rigging:** Hyper3D Rodin offers neither rigging nor animation. Tencent's HY-Motion is open weights only.

### For L6: region and mask edits

| Model | Targeting | Pixels outside the edit, in the vendor's words | Price |
|---|---|---|---|
| **Ideogram 4.5 Precise Edit**, `POST https://api.ideogram.ai/v2/image/precise-edit/ideogram-4-5` ([docs](https://developer.ideogram.ai/api-reference/images/precise-edit/ideogram-4-5)) | A box through `context_window` (`y_min,x_min,y_max,x_max`, or `auto`); an optional mask, **black = edit**; up to 4 references | "pixels the edit did not meaningfully change are **copied exactly**" | Priced by `quality`; `dry_run` returns a quote, with no public dollar figure. Sync, or `async` with `webhook_url` |
| **Bria** `/v2/image/edit`, `/erase`, `/remove_object` ([docs](https://docs.bria.ai/image-editing-overview)) | A mask, or text for `/remove_object` | "without altering the surrounding pixels" | **(unverified)** |
| **BFL FLUX.1 Fill [pro]**, `POST https://api.bfl.ai/v1/flux-pro-1.0-fill` ([docs](https://docs.bfl.ml/flux_1_fill.md)) | A mask, **white = edit**, or transparent pixels | "black areas will be preserved" | $0.05; async, results kept 10 minutes |
| **BFL FLUX 3 Image**, `POST /v1/flux-3-image` ([docs](https://docs.bfl.ml/flux_3/flux3_image_bounding_boxes.md)) | Boxes in the prompt JSON: `src_bbox` to `tgt_bbox`, as `[top,left,bottom,right]` on a 0–1000 scale | "usually stay identical"; boxes "are not clipping masks" | From $0.048 ([pricing](https://docs.bfl.ml/quick_start/pricing)) |
| **Stability Inpaint**, `POST /v2beta/stable-image/edit/inpaint` ([docs](https://docs.stability.ai/api-reference/edit/inpaint.md)) | A mask, **white = edit**, or alpha. `grow_mask` (default 5) grows and blurs the edge, so send 0 | No promise | $0.05; Erase and Search-and-Replace are also $0.05 ([pricing](https://docs.stability.ai/pricing.md)) |
| **Recraft**, `POST /images/inpaint` ([docs](https://www.recraft.ai/docs/api-reference/endpoints)) | A mask; convention not stated | Not stated | $0.04 or $0.08; `eraseRegion` $0.002 |
| **OpenAI GPT Image 2.5**, `POST /v1/images/edits` ([docs](https://developers.openai.com/api/docs/guides/image-generation)) | An alpha mask the size of the image, **transparent = edit** | "entirely prompt-based… may not follow its exact shape" | $8 per 1M image-input tokens, $30 per 1M image-output tokens |

- **Conventions:** this table holds three of them (black, white and transparent mean edit). That is why L6 asks litegen to accept one and convert it for each vendor.
- **`preserves_unmasked: true`** fits Ideogram 4.5 and Bria. Every other model gets `false`.
- **The first real `region` model:** Ideogram 4.5 Precise Edit. It takes a box natively, and litegen already has an Ideogram adapter. The cheapest mask models, FLUX.1 Fill and Stability Inpaint, also fit existing adapters.
- **No mask or box at all:** these get `supports_inpainting: false` and `supports_region_edit: false`, and PixelGenie crops for them.
  - Google's Gemini image models. Google's "semantic masking" means describing the region in words.
  - Qwen Image 2.0.
  - Seedream 5.0 **(unverified)**.

### For L7: promptable segmentation

- **fal SAM 3**, [`fal-ai/sam-3/image`](https://fal.ai/models/fal-ai/sam-3/image): $0.005 per request, and fal is already enabled.
  - Prompts: a text `prompt`, `point_prompts`, and `box_prompts` as `x_min,y_min,x_max,y_max`. Boxes that share an `object_id` form one object.
  - Options: `return_multiple_masks`, `max_masks` (up to 32), `include_scores` and `include_boxes`.
  - It returns mask images. **Its boxes come back as `[cx,cy,w,h]`**, unlike its input, so convert them to `{ x, y, w, h }`.
  - SAM 3.1 is at [`fal-ai/sam-3-1/image-rle`](https://fal.ai/models/fal-ai/sam-3-1/image-rle) for $0.01.
- **Roboflow serverless SAM 3** ([docs](https://docs.roboflow.com/deploy/supported-models/sam3)):
  - `POST https://serverless.roboflow.com/sam3/concept_segment` takes text or example boxes (up to 16 prompts) and returns polygons or RLE.
  - `/sam3/visual_segment` takes points or a box for one object.
  - Billed per image since 2026-09-18; the SAM 3 rate wasn't found.
- Meta publishes SAM 3 as open weights only, with no hosted API.
- **Start with fal SAM 3.** It covers all three of L7's prompt types for half a cent.

### For L8 and L9: background removal and native transparency

| Background removal | Price | Notes |
|---|---|---|
| [`fal-ai/bria/background/remove`](https://fal.ai/models/fal-ai/bria/background/remove) (RMBG 2.0) | $0.018, or $0.03 with a prompt | fal is already enabled |
| [`fal-ai/birefnet/v2`](https://fal.ai/models/fal-ai/birefnet/v2) | **(unverified)**; the page lists "$0 per compute second" | Variants General Light, Light 2K, Heavy, Dynamic, Matting and Portrait. `operating_resolution` 1024, 2048 or 2304; `output_mask`, `refine_foreground` and `mask_only` |
| Recraft `removeBackground` | $0.01 | Existing adapter |
| Stability remove-background | $0.05 | Existing adapter; LiteLLM's example (section 8) |
| PixelLab `/v2/remove-background` | About $0.0055 | Up to 400 px |
| Retro Diffusion `background_remover` | $0.01 | |

Models that return true transparency, for L9's `background: "transparent"`:
- GPT Image 2.5 (`sunburst` and `flare`, both listed) with `background: "transparent"` and PNG or WebP output. Third-party sources say `gpt-image-2` can't.
- Ideogram 3 at `/v2/image/generate/ideogram-3-transparent`, but not at the `FLASH` speed ([docs](https://developer.ideogram.ai/api-reference/api-reference/generate-transparent-v3)).
- PixelLab's pixflux, bitforge, inpaint and Pro Flash, and Retro Diffusion with `remove_bg` (L16).
- Gemini and FLUX document no transparent output, and Recraft's docs don't mention it.

### For L10: video models for sprites

| Model | In litegen | First and last frame | Still camera or loop | Lengths | Cheapest published rate |
|---|---|---|---|---|---|
| **Seedance 1.0 Pro** ([docs](https://docs.byteplus.com/en/docs/ModelArk/1520757)) | `bytedance/doubao-seedance-1-0-pro-250528`, base64 | Yes; identical frames allowed | **`camera_fixed: true`**, 1.x models only, "not guaranteed" | 2–12 s | About $0.12 for 5 s at 480p **(computed)** from $2.5 per 1M tokens. litegen bills a flat $0.61 (L14) |
| **MiniMax Hailuo-02** ([docs](https://platform.minimax.io/docs/api-reference/video-generation-fl2v)) | `minimax/MiniMax-Hailuo-02`, base64 | Yes; no 512P | **`[Static shot]`** in the prompt | 6 or 10 s | $0.28 for 6 s at 768P |
| **MiniMax H3-Lite** ([docs](https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md)) | Not listed; H3 and H3-Max are | Yes | Prompt only | 4–15 s | $0.025/s at 480P ([pricing](https://platform.minimax.io/docs/guides/pricing)) |
| **PixVerse V6 and C1** ([capabilities](https://docs.platform.pixverse.ai/model-capability-map-2394982m0)) | `pixverse/v6`, `pixverse/c1`; litegen uploads the frames | Yes, except V6 Max | Prompt only | V6: 1–15 s | $0.022/s at 360p with audio off ([pricing](https://docs.platform.pixverse.ai/pixverse-api-pricing-2394995m0)) |
| **Wan 2.2 kf2v flash** ([docs](https://www.alibabacloud.com/help/en/model-studio/image-to-video-by-first-and-last-frame-api-reference)) | Not listed; no Alibaba adapter | `first_frame_url` and `last_frame_url` | Prompt only | 5 s only | $0.015/s at 480P, so $0.075 a clip **(computed)** |
| **Kling 2.5 Turbo and 3.0** ([docs](https://kling.ai/document-api/apiReference/model/imageToVideo)) | `kling/video-kling-v2-5-turbo`, `kling/video-kling-v3`, base64 | Yes; 2.5 Turbo only at 1080p | Prompt only | 5 or 10 s; 3.0: 3–15 s | 2.5 Turbo $0.07/s at 1080p; 3.0 $0.084/s at 720p ([pricing](https://kling.ai/document-api/pricing/base/video)) |
| **Seedance 2.0 Mini** | `bytedance/dreamina-seedance-2-0-mini-260615`, base64 | Yes | No `camera_fixed` | 4–15 s | $0.18 for 5 s at 480p |
| **Vidu Q3 turbo** ([docs](https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/start-end-to-video)) | `vidu/viduq3-turbo`, URL only (L4) | Yes | Prompt only; **audio is on by default** | 1–16 s | 7 credits/s at 540p; 1 credit = RMB 0.03125 |
| **Luma `ray-3.2`** ([docs](https://docs.agents.lumalabs.ai/guides/videos/generation)) | Not listed; litegen has `ray-2` on the old API | Yes | **`loop: true`**, but not with `end_frame` or 10 s | 5 or 10 s | 20 credits for 5 s at 360p draft; a credit's price depends on the plan |
| **Gemini Omni Flash**, `gemini-omni-1.1-flash` ([docs](https://ai.google.dev/gemini-api/docs/omni)) | Not listed; Google's replacement for the Veo 3.1 previews | `<FIRST_FRAME>` and `<LAST_FRAME>` prompt tags | **The same image in both tags makes a loop** (documented) | Not found | About $0.10/s at 720p |
| **Veo 3.1 Lite and Fast**, GA on Agent Platform ([docs](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/veo/3-1-generate)) | Not listed; litegen's Veo ids are the shut-down Gemini API previews | Yes | Prompt only | 4, 6 or 8 s | Lite $0.03/s (preview), Fast $0.08/s, at 720p without audio |
| **Runway `gen4_turbo`** ([docs](https://docs.dev.runwayml.com/api/)) | `runway/gen4-turbo` | First frame only | Prompt only | 2–10 s | $0.05/s |

- **Still camera and loops:** only Seedance 1.x (`camera_fixed`) and Hailuo-02 (`[Static shot]`) have a still-camera control. Only Luma has a loop flag; Omni and Seedance document loops made from identical first and last frames.
- **PixelGenie's interim video default is Seedance 1.0 Pro.** It is listed, takes base64 frames and a last frame, and has a still camera. It needs the ByteDance provider enabled (section 11, question 1).
- **The cheapest first-and-last-frame clips:**
  - Wan 2.2 kf2v: $0.075 for 5 s.
  - PixVerse V6: $0.022/s, from 1 s.
  - MiniMax H3-Lite: $0.10 for 4 s.
  - Seedance 1.0 Pro: about $0.12 for 5 s.

### For L11: multiview images and multiview-to-3D

- **Tripo** ([image-to-multiview](https://developers.tripo3d.com/en/docs/generation-image-to-multiview.md), [multiview-to-model](https://developers.tripo3d.com/en/docs/generation-multiview-to-model/p.md)):
  - `POST /v3/generation/image-to-multiview` returns front, left, back and right views for 10 credits.
  - `POST /v3/generation/multiview-to-model` costs 20 or 30 credits. It takes `inputs` keyed by view, or the multiview `task_id`; front is required, with at least 2 views.
  - Its models are `P1-20260311` (low-poly), `P2-20260801` (quads) and the default `v3.1-20260211`.
  - On fal as [`tripo3d/h3.1/multiview-to-3d`](https://fal.ai/models/tripo3d/h3.1/multiview-to-3d): $0.20 untextured, $0.30 textured, $0.40 HD. Its `task_id` goes straight to Tripo's rig-check and rig (L5).
- **Meshy** ([text-to-image](https://docs.meshy.ai/en/api/text-to-image), [multi-image-to-3d](https://docs.meshy.ai/en/api/multi-image-to-3d)):
  - `generate_multi_view: true` on text-to-image or image-to-image returns 3 views for 3–12 credits. It can't be combined with `aspect_ratio`.
  - `POST /openapi/v1/multi-image-to-3d` takes 1–4 `image_urls` or the multiview `input_task_id`, for 20–35 credits.
- **Tencent HY 3D Pro**, [`SubmitHunyuanTo3DProJob`](https://www.tencentcloud.com/document/api/1284/75540):
  - `MultiViewImages` takes left, right and back views; model 3.1 adds top, bottom, left_front and right_front.
  - One JPG or PNG per view, up to 8 MB and 128–5000 px.
- **Hyper3D Rodin Gen-2.5** ([docs](https://docs.hyper3d.ai/en/api-specification/rodin-gen2-5)) takes 1–5 images. Check whether `fal/hyper3d-rodin` can pass several.
- **New views of a flat image:** fal's [`fal-ai/qwen-image-edit-plus-lora-gallery/multiple-angles`](https://fal.ai/models/fal-ai/qwen-image-edit-plus-lora-gallery/multiple-angles) rotates, tilts and zooms the camera; its schema is **(unverified)**. The pixel-art rotations are under L16 below.
- **Start with Tripo on fal.** fal is enabled, and the result chains into Tripo's rigging.

### For L16: pixel-art vendors

**PixelLab v2**, `https://api.pixellab.ai/v2` with a Bearer key ([docs](https://api.pixellab.ai/v2/llms.txt), [pricing](https://www.pixellab.ai/pixellab-api)). Jobs are async: poll `GET /background-jobs/{id}`. Prices are PixelLab's own per-call estimates.

| Kind | Endpoint | Sizes and price |
|---|---|---|
| Generate | `create-image-pixflux` | Up to 400 px square, $0.008–0.013 |
| | `create-image-bitforge`, which takes a style image | Up to 200 px square |
| | `create-image-pixen` | Up to 512 px square |
| | Pro | $0.08, $0.115 or $0.15 for up to 256, 341 or 512 px |
| | Pro Flash | Up to 256 px, $0.024–0.033 |
| Directions | `create-character-with-4-directions`, `-8-directions` | 48–64 px, $0.011–0.017 |
| | `create-character-v3` | Up to 168 px, $0.041–0.045 |
| | `generate-8-rotations-v3`, which turns a south-facing image into 8 directions | Up to 256 px, $0.029–0.038 |
| Animate | `estimate-skeleton`, then `animate-with-skeleton-v3` | 3–15 frames, up to 256 px; $0.005, then $0.044–0.062 |
| | `animate-with-text-v3` | Up to 16 frames and 256 px, $0.022–0.042 |
| | `animate-pixminimax` | 4–40 frames; needs a Tier 1 subscription |
| Edit | `inpaint`, `inpaint-v3` (Pro), Pro Flash inpaint | Up to 200 px; Pro Flash 32–256 px |
| Tiles | `create-tileset` (top-down), `create-tileset-sidescroller` | 16 or 32 px tiles, $0.008 or $0.010 |
| | `create-isometric-tile` | 16–64 px, $0.016–0.017 |

- **Forced palette:** works on pixflux, bitforge, animate-with-text, animate-with-skeleton, rotate, inpaint and the isometric tile.
- **Utilities:** `reduce-colors`, `remove-background`, `image-to-pixelart` and `normal-map`.

**Retro Diffusion**, `POST https://api.retrodiffusion.ai/v2/inferences` with an `X-RD-Token` header ([docs and examples](https://github.com/Retro-Diffusion/api-examples)). Every v2 job is queued: poll `GET /v2/inferences/tasks/{task_id}`. Failed jobs are refunded.

| Kind | Style | Sizes and price |
|---|---|---|
| Generate | `rd_fast__*` | 64–384 px, $0.03 |
| | `rd_plus__*`, including `character_turnaround`, `topdown_asset`, `isometric_asset` and `item_sheet` | 64–384 px, $0.06 |
| | `rd_pro__*`, with up to 9 `reference_images` | 12–256 px for platformer, topdown and isometric; 64–256 px for spritesheet; $0.18 |
| Tiles | `rd_tile__tileset` | 16–32 px tiles, $0.10 |
| Animate a still | `rd_advanced_animation__walking`, `idle`, `jump`, `crouch`, `attack` and `destroy` | 32–256 px input, $0.14 |
| | `custom_action` and `subtle_motion` | $0.25 |
| | `rotate`, which returns 8 frames | $0.10 |
| Animate from a prompt | `rd_animation__four_angle_walking` (48 px), `small_sprites` (32 px) and `vfx` | $0.07 |
| | `any_animation`, `big_animation` and `8_dir_rotation` | $0.25 |
| Edit | Inpainting and outpainting | $0.18 |
| | `palette_converter`, `color_reducer` and `k_centroid_downscale` | $0.001 |

- **Parameters PixelGenie would use:**
  - `input_palette`, with `return_pre_palette`;
  - `remove_bg`;
  - `tile_x` and `tile_y`;
  - `frames_duration` (4–16);
  - `return_spritesheet`;
  - `check_cost`, a free dry run.
- **Upscaling:** leave `upscale_output_factor` unset (L16).
- **Output:** base64 PNG or GIF, or `output_urls`.

Ludo.ai and Scenario also have APIs, but they aren't specific to pixel art, and the scan couldn't verify them.

## 8. Prior art in LiteLLM

A LiteLLM checkout sits next to litegen at `../litellm` (upstream BerriAI/litellm, branch `litellm_internal_staging`). It has no 3D and no segmentation, but its image and video layers have solved several of the problems above.

**Worth porting:**

1. **Each edit operation exposed as its own model.** Stability's 14 operations, Bedrock's 13, and Nova picking inpainting or variation by whether a mask is present. Background removal and upscaling arrive this way. Read `litellm/types/llms/stability.py` and `litellm/llms/bedrock/image_edit/`.
2. **One job API over Veo operations, Runway tasks and Sora jobs.** Read `litellm/llms/base_llm/videos/transformation.py` and each provider's `videos/transformation.py`.
3. **Job ids that carry the provider and model, with an access check on every follow-up call.** Read `litellm/types/videos/utils.py` and `litellm/proxy/auth/auth_utils.py:1229`. Do better than LiteLLM: sign the id and pin the exact deployment.
4. **Media pricing, with the price reserved against the budget before the call.**
   - Prices per image, per pixel, per token and per second, with resolution tiers.
   - Read `litellm/cost_calculator.py:1907` and `:2008`, `litellm/proxy/spend_tracking/budget_reservation.py:868`, and `model_prices_and_context_window.json`.
   - This also covers your per-megapixel FLUX pricing.
5. **Polling inside the gateway, so a slow vendor looks synchronous.** It also checks that the polling URL the vendor returns is on the vendor's own host. Read `litellm/llms/black_forest_labs/*/handler.py` and `litellm/litellm_core_utils/url_utils.py:321`.
6. **Masks described in text.** Nova's `maskPrompt`, and Stability's `search_prompt` and `select_prompt`. Read `litellm/llms/bedrock/image_edit/amazon_nova_canvas_image_edit_transformation.py`.

**Not worth copying:**
- Silent defaults: unknown sizes become 1:1 or 16:9, and `n` is ignored.
- Edit parameters dropped without an error.
- No normalising of mask conventions; LiteLLM never touches mask pixels.
- Whole videos buffered in memory.
- Video billed when the job is created, even if it later fails.
- Failed Gemini Veo jobs reported as completed (`litellm/llms/gemini/videos/transformation.py:433`).

## 9. What PixelGenie does itself

Don't build these for PixelGenie:
- reducing images to the native grid, and snapping to the palette;
- compositing edits back while keeping locked pixels;
- the crop fallback for models that take neither masks nor boxes;
- extracting video frames with ffmpeg;
- rendering GLBs into facings, and mapping joints to parts;
- spend caps and the ledger.

It also follows a few fixed habits:
- It polls, and needs no webhooks.
- It sends one image per call.
- It keeps strict mode on.

When these requests land, PixelGenie will change its side as follows:

| Request | PixelGenie change |
|---|---|
| L1 | Send its ledger key as `Idempotency-Key` |
| L3 | Download through the assets route |
| L4 | Send base64 to URL-form models, and drop `publicBaseUrl` |
| L6 | Read `mask_convention`, and send `region` as a real field when `supports_region_edit` is set |
| L5 | Add `animations` to its 3D request |
| L14 | Read retirement fields instead of its own list |

## 10. Promises PixelGenie needs

- Changes to the flags in section 2 and the fields PixelGenie reads are additive. A rename keeps the old name for at least one release.
- `catalog_version` changes whenever any of them changes.
- `GET /v1/generations/{id}` stays the durable way to read a job.
- The mock model ids in section 6 stay stable.

## 11. Questions for litegen

1. Can OpenAI, Stability and ByteDance be enabled? The local `litegen.yaml` enables none of the three.
   - OpenAI and Stability are the only vendors that take masks today.
   - ByteDance's Seedance 1.0 Pro is the only listed video model with base64 frames, a last frame and a still-camera setting, so it is PixelGenie's interim video default.
2. Which vendor accounts exist, or can be added, for rigging and animation, segmentation, background removal and pixel art?
3. Is S3 storage configured on `litegen.visgotti.com`? Without it there are no result URLs, and 3D files don't survive a restart.
4. Will PixelGenie get its own API key, and with what quota?
5. When are L1 to L6 expected? PixelGenie's 3D characters and region edits depend on them.
