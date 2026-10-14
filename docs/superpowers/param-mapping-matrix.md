# Param mapping matrix

**What this is:** for every parameter litegen advertises, where it comes from,
what validates it, what it becomes on the wire to each provider, and what test
proves that. Tick a row only when you have SEEN the mapping work — a green unit
test on the validator is not evidence the provider sends the right field name.

**Legend:** ✅ verified (test named) · 🟡 implemented, unverified against a live
vendor · ⬜ not implemented · — not applicable to this family

**How this was generated:** the row inventory (media_type, param, kind) is the
literal output of a one-off script over `models/*.yaml` (recorded below), not
hand-transcribed. The ✅/🟡/⬜ status per row was assigned by reading the actual
test files named in each cell — `src/api/middleware/validator_tests.rs`,
`tests/catalog_conformance.rs`, and each provider's `#[cfg(test)] mod tests` —
not by inspecting the implementation and assuming it is covered.

## How a request flows

```
  POST /v1/{family}/generations
    → Validated{Image,Video,Model3d} extractor        (validator.rs)
        · deserialize into the family's request struct
        · check_prompt / per-param spec check
        · strict=true  → 4xx `param_unsupported`
          strict=false → drop + record in X-Litegen-Dropped-Params
        · check_refs   (roles vs. ref_inputs)
        · check_extra  (keys vs. extra_allowlist)
    → materializer   (ref images → url | base64 | multipart per provider_format)
    → {Image,Video,Model3d}Extras                     (handlers/mod.rs)
    → router dispatch → provider.generate(schema, base, extras, materialized)
    → provider maps extras → the vendor's own field names   ← THE MAPPING
    → async families: handle stored, poller drives to terminal
```

## Verification recipe

For ONE (param, provider) cell, ✅ requires all four that apply:
1. The model YAML advertises it with the right `ParamSpec` kind.
2. A validator test asserts both strict-reject and lax-drop for it.
3. A provider test (wiremock or unit) asserts the OUTBOUND field name and value.
4. `catalog_conformance` asserts any enum values against the vendor's documented set (n/a for non-enum kinds).

Name the test in the cell, e.g. `stability::tests::image_to_image_sends_mode_image_to_image`.

**A structural note on criterion 2, read before trusting any ✅ below:**
`validate_image`'s per-param dispatch is one shared macro (`check_param!` in
`src/api/middleware/validator.rs`). Its "model doesn't declare this param at
all → strict rejects / lax drops" branch is IDENTICAL generated code for every
param that goes through it, and is exercised by exactly one pair of tests,
`unsupported_param_strict_rejects` / `unsupported_param_lax_drops`, using
`steps` as the literal example. Once a param IS declared, its *value* rule
(range/enum/length) always errors regardless of `strict` — there is no lax
variant for an out-of-range value, so "strict-reject and lax-drop" as literally
written cannot apply to a value-rule test. Below, ✅ on a range/enum/length
param means: a test named in the cell asserts that specific value rule
(param-specific, not the shared drop mechanism) AND a provider test proves the
outbound mapping. The shared drop mechanism itself is proven exactly once, via
`steps`, and is **not** re-claimed as separate evidence for every other row —
doing so would be exactly the "looks right so I ticked it" mistake this doc
exists to prevent.

**`validate_video` has no dedicated test file at all** — `validator_tests.rs`
only builds `ImageGenerationRequest`s. Every video param below is therefore
capped at 🟡 regardless of how well its provider mapping is proven, because
criterion 2 is unmet for the whole family. This is a real, no-code-change gap
worth a follow-up test file (`validate_video` unit tests mirroring
`validator_tests.rs`'s image coverage).

## Image / Video

Row inventory generated 2026-08-27 via:

```python
import glob, yaml, collections
rows = collections.defaultdict(set)
for f in sorted(glob.glob('models/*.yaml')):
    doc = yaml.safe_load(open(f))
    for m in doc.get('models') or []:
        for name, spec in (m.get('params') or {}).items():
            rows[(m['media_type'], name, spec.get('kind'))].add(m['id'])
for (media, name, kind), ids in sorted(rows.items()):
    print(f"{media}\t{name}\t{kind}\t{len(ids)}\t{sorted(ids)[0]}")
```

Output — 24 rows, all reproduced below (9 image, 8 model3d, 7 video; the
model3d rows are re-tabulated in the dedicated 3D section further down, not
repeated here):

```
image   aspect_ratio     aspect_ratio  26  bfl/flux-kontext-max
image   guidance_scale   float         12  bedrock/amazon.nova-canvas-v1:0
image   negative_prompt  string        22  bedrock/amazon.nova-canvas-v1:0
image   quality          string         4  bedrock/amazon.nova-canvas-v1:0
image   seed             seed          39  bedrock/amazon.nova-canvas-v1:0
image   size             size          25  bedrock/amazon.nova-canvas-v1:0
image   steps            int           13  fal/auraflow
image   strength         float          3  leonardo/diffusion-xl
image   style            string        11  fal/recraft-v3
video   aspect_ratio     aspect_ratio  24  bytedance/doubao-seedance-1-0-lite-i2v-250428
video   duration_seconds float        35  bedrock/amazon.nova-reel-v1:1
video   duration_seconds int           2  runway/gen4-turbo
video   fps              int            3  bedrock/amazon.nova-reel-v1:1
video   negative_prompt  string         3  kling/video-kling-v1-6
video   resolution       string        23  bedrock/amazon.nova-reel-v1:1
video   seed             seed          10  bedrock/amazon.nova-reel-v1:1
```

### Image

| param | kind | validator rule | # models | Proof | Status |
|---|---|---|---|---|---|
| `aspect_ratio` | AspectRatio | enum `allowed` | 26 | validator: `aspect_ratio_not_allowed`. provider: `ideogram::tests::generates_ideogram_v3_json` (`body["aspect_ratio"]`), `minimax::tests::generates_image_01`, `luma::tests::generates_photon_image_via_polling`, `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["aspect_ratio"]`). conformance: `runway_image_does_not_advertise_unmappable_ratios`, `stability_aspect_ratios_are_in_the_v2_enum`, `minimax_image_aspect_ratios_match_the_documented_enum`. | ✅ |
| `guidance_scale` | Float | range | 12 | validator: `float_out_of_range`. provider: `fal::tests::generates_flux_dev_image` (`body["guidance_scale"]==3.5`), `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["guidance"]==3.5`) and `replicate::tests::guidance_and_steps_use_each_models_own_input_names` (each model's own input name: FLUX `guidance`, SD3 `cfg`, fallback `guidance_scale`; corrected 2026-09-11, the adapter used to send `guidance_scale` to every model). conformance: n/a (not enum). | ✅ |
| `seed` | Seed | range | 39 | validator: `seed_out_of_range`. provider: `fal::tests::generates_flux_dev_image` (`body["seed"]==42`), `bfl::tests::generates_flux_pro_via_polling_url` (`body["seed"]==7`), `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["seed"]==123`). conformance: n/a. | ✅ |
| `size` | Size | enum of `[w,h]` pairs | 25 | validator: `size_enum_must_match`, `size_enum_passes_when_matches`. provider: `bytedance::tests::generates_seedream` (`body["size"]=="2048x2048"`), `openai::tests::generates_dalle3_image_b64_json` (`body["size"]=="1024x1024"`), `recraft::tests::generates_recraftv3_image` (`body["size"]=="1024x1024"`). conformance: **no test** asserts a model's `size` dimension list against the vendor's accepted sizes — real gap. | 🟡 |
| `quality` | String enum | enum + max_length | 4 | validator: **none** — no `validator_tests.rs` case sends `req.quality`. provider: `openai::tests::generates_dalle3_image_b64_json` (`body["quality"]=="high"`). conformance: `openai_image_quality_values_are_the_gpt_image_enum`. | 🟡 |
| `style` | String enum | enum + max_length | 11 | validator: **none** — no case sends `req.style`. provider: `recraft::tests::generates_recraftv3_image` (`body["style"]=="digital_illustration"`); `openai::tests::generates_dalle3_image_b64_json` proves the *absence* case (`style` omitted for dall-e-3). conformance: `ideogram_style_values_match_the_documented_enum`. | 🟡 |
| `negative_prompt` | String | max_length | 22 | validator: **none**. provider: `stability::tests::generates_sd3_large_via_v2_multipart` (asserts the multipart body contains the negative-prompt text, not a strict field-name+value pair). conformance: n/a. | 🟡 |
| `strength` | Float | range | 3 | validator: **none** — no `strength_out_of_range` case exists (only `guidance_scale` is exercised for the Float kind). provider: strong — `stability::tests::image_to_image_sends_mode_image_to_image`, `strength_is_not_sent_for_text_to_image`, `image_to_image_defaults_strength_when_omitted` all assert presence/absence of the outbound `strength` multipart field. conformance: n/a. | 🟡 |
| `steps` | Int | range | 13 | validator: `unsupported_param_strict_rejects` / `unsupported_param_lax_drops` prove the shared drop mechanism using `steps` as the literal example — this is genuine, param-named coverage, unlike the other rows above. No test exercises the *range* branch (`steps` out of `[min,max]`) specifically. provider: `fal::tests::generates_flux_dev_image` (`body["num_inference_steps"]==28` — **note the outbound rename `steps` → `num_inference_steps`**), `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["num_inference_steps"]==28`) and `replicate::tests::guidance_and_steps_use_each_models_own_input_names` (each model's own input name: flux-dev `num_inference_steps`, flux-pro and SD3 `steps`, fallback `num_inference_steps`). Not universal: `stability.rs` sets `body["steps"]` (line 466) and the multipart `steps` field (line 375) with zero assertion. conformance: n/a. | 🟡 |

### Video

Every row here is capped at 🟡 by the family-wide validator gap noted above,
even where provider evidence is strong.

| param | kind | validator rule | # models | Proof | Status |
|---|---|---|---|---|---|
| `duration_seconds` | Float (35 models) / Int (2 models) | range | 37 | validator: none (`validate_video` untested). provider: `runway::tests::submits_runway_job_and_returns_handle` (`body["duration"]==5`) — **note the outbound key is `duration`, not `duration_seconds`; this row is exactly the kind of silent rename this doc exists to surface**. conformance: `vidu_q1_duration_and_resolution_match_the_model_map`, `bedrock_nova_reel_duration_matches_the_text_video_task`. | 🟡 |
| `resolution` | String enum | enum | 23 | validator: none. provider: `google::tests::submits_veo_job_and_sends_api_key_header` (`body["parameters"]["resolution"]=="720p"`), `leonardo::tests::submits_image_to_video_and_polls` (`body["resolution"]=="RESOLUTION_720"`). conformance: `vidu_q1_duration_and_resolution_match_the_model_map`. | 🟡 |
| `aspect_ratio` | AspectRatio | enum `allowed` | 24 | validator: none. provider: setters exist in `kling.rs`, `luma.rs`, `pixverse.rs`, `vidu.rs`, `fal.rs`; the only *assertion* found is a negative case, `fal::tests::*` (`body.get("aspect_ratio").is_none()` for ltx-video, which doesn't support it) — **no test proves the correct value reaches the body for a model that does support it**. conformance: `runway_video_aspect_ratios_are_in_the_gen4_enum`. | 🟡 |
| `seed` | Seed | range | 10 | validator: none. provider: setters exist in `bedrock.rs`, `vidu.rs`, `pixverse.rs`, `runway.rs`, `fal.rs`, `replicate.rs` — **zero assertions on any of them**; a regression that dropped or mis-mapped video seed would go unnoticed by the suite. conformance: n/a. | 🟡 |
| `negative_prompt` | String | max_length | 3 | validator: none. provider: setters exist in `kling.rs`, `pixverse.rs` — **zero assertions**. conformance: n/a. | 🟡 |
| `fps` | Int | range | 3 | validator: none. provider: `bedrock.rs` sets `video_config["fps"]` — **zero assertions anywhere in the suite**. conformance: n/a. | 🟡 |

## 3D (`model3d`)

**fal is the implemented 3D vendor** — `fal/hyper3d-rodin`, `fal/trellis` and
`fal/hunyuan3d-v2`, mapped by `litegen-core/src/providers/model3d/fal.rs`.
Every cell below was read from that endpoint's own schema,
`https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={endpoint}`
(re-read 2026-10-12), and is proven by name: a wiremock test asserting the
OUTBOUND field (`fal::tests::…`), a validator test against the shipped catalog
(`model3d_validation::…`), and the test that ties each `models/fal.yaml` row to
its arm of the adapter's endpoint table,
`fal::tests::each_catalog_row_declares_exactly_what_its_endpoint_takes` — so the
yaml and the adapter cannot drift apart silently. "—" means the endpoint's
schema has no such field: the row does not declare the param, and validation
refuses it (strict) or drops and reports it (lax) before the adapter runs.

| litegen param | ParamSpec | `fal-ai/hyper3d/rodin` | `fal-ai/trellis` | `fal-ai/hunyuan3d/v2` | Proof | Status |
|---|---|---|---|---|---|---|
| `prompt` | PromptSpec | `prompt`, ≤ 1024 chars; required when there is no image | — (`max_length: 0`) | — (`max_length: 0`) | `rodin_text_to_3d_sends_rodins_own_field_names` (`body["prompt"]`); `trellis_takes_image_url_and_never_a_prompt_or_a_format` and `hunyuan_takes_input_image_url_and_maps_texture_to_textured_mesh` (no `prompt` key); validator `rodins_prompt_limit_is_1024_characters_not_bytes`, `image_only_models_take_no_prompt`, `text_to_3d_needs_a_prompt_even_where_the_prompt_is_optional` | ✅ |
| `init` image(s) | RefInputSpec (`form: url`) | `input_image_urls[]` (≤ 5) | `image_url` (required) | `input_image_url` (required) | `rodin_image_to_3d_sends_every_image_as_an_array_and_no_empty_prompt`; trellis/hunyuan tests above; `an_uploaded_image_in_base64_form_is_sent_as_a_data_uri`; validator `image_only_models_require_their_image` | ✅ |
| `output_formats` | StringArray, `max_items: 1` | `geometry_file_format` — glb as the conversion source, or the vendor-native container asked for (fbx, usdz: `prefers_vendor_native`) | — (glb only) | — (glb only) | `rodin_text_to_3d_sends_rodins_own_field_names` (`glb` for `[glb, obj]`); `rodin_is_asked_for_its_own_fbx_rather_than_a_conversion` (`fbx`); trellis test (no format key); validator `rodin_serves_the_derivable_set_from_one_job_but_not_two_native_formats`, `glb_only_models_deliver_the_whole_derivable_set`; live, 2026-09-12 (below) | ✅ |
| `seed` | Seed | `seed`, 0..=65535 | `seed`, u32 | `seed`, u32 | rodin and trellis tests (`body["seed"]`); validator `rodins_seed_ceiling_is_its_u16_maximum`; adapter refuses rather than clamps: `requests_fal_would_only_reject_are_refused_before_submitting` | ✅ |
| `pbr` | Bool | `material`: `PBR` / `Shaded` | — | — | `rodin_text_to_3d_sends_rodins_own_field_names` (`PBR`); `rodin_is_asked_for_its_own_fbx_rather_than_a_conversion` (`Shaded`); validator `texture_and_pbr_go_only_where_the_vendor_has_a_field_for_them` | ✅ |
| `texture` | Bool | — | — | `textured_mesh` (fal charges 3×) | `hunyuan_takes_input_image_url_and_maps_texture_to_textured_mesh`; validator `texture_and_pbr_go_only_where_the_vendor_has_a_field_for_them` | ✅ |
| `target_polycount`, `topology`, `symmetry`, `rig`, `negative_prompt` | Int / String / Bool | — | — | — | refused in strict, dropped and cleared in lax: `every_3d_param_is_refused_in_strict_and_dropped_in_lax_where_undeclared` (on TRELLIS); honoured where declared: `every_declared_3d_param_passes_through_on_the_model_that_declares_it` (mock) | ✅ (refusal) |
| `extra` (allowlisted) | `extra_allowlist` | quality, tier, use_hyper, TAPose, condition_mode, bbox_condition, addons | mesh_simplify, texture_size, ss_*, slat_* | num_inference_steps, guidance_scale, octree_resolution | trellis and hunyuan tests (passed through verbatim); `an_extra_field_never_overrides_a_canonical_one`; validator `a_vendor_field_outside_the_allowlist_is_refused_in_strict_mode` | ✅ |

The vendor's answers are proven the same way: `queue_states_map_to_pending_and_processing`,
`completed_downloads_the_mesh_and_rodins_textures_with_their_dimensions`,
`completed_with_an_error_is_failed_with_fals_message_and_type`,
`a_stored_error_result_is_fals_verdict_but_a_gateway_blip_is_not` (the 422
`detail`-array and `error_type` paths the live runs could not reach),
`a_submit_error_carries_fals_message_and_its_retry_verdict`,
`a_transient_status_failure_stays_retryable`,
`a_definitive_status_failure_is_not_retried_forever`,
`a_mesh_that_will_not_download_fails_or_retries_by_cause`,
`asset_urls_must_resolve_to_a_public_address`,
`the_key_only_goes_to_the_configured_queue_origin` and
`cancel_maps_fals_three_answers`.

### Proof: real billed runs against fal, 2026-09-12

Four generations across all three fal 3D models, plus six rejections that had to
cost nothing. ~$0.98 quoted. Everything below was validated by downloading the
bytes, not by trusting the sizes in the response.

| run | model | mode | asked for | delivered |
|---|---|---|---|---|
| A | `fal/hyper3d-rodin` | image-to-3d | glb, obj, stl, ply | glb (vendor) + obj, stl, ply (derived) |
| B | `fal/hyper3d-rodin` | **text**-to-3d | **fbx** | fbx (vendor) + **5 texture PNGs** |
| C | `fal/hunyuan3d-v2` | image-to-3d | glb, stl | glb (vendor) + stl (derived) |
| D | `fal/trellis` | image-to-3d | glb, obj | glb (vendor) + obj (derived) — **terminalised by the background poller, never client-polled** |

**Run A** — 35,394 triangles and 23,125 vertices identical across obj, ply and
stl; glb magic and declared length exact; obj free of `mtllib`/`usemtl`; stl
`84+50n` and not `solid`-prefixed; ply `binary_little_endian` with matching
header counts.

**Run B is the one that matters most.** It proves three things nothing else did:

- The **text-to-3D arm** works with no reference image at all.
- A container litegen **cannot derive** is asked of the vendor. This caught a
  real bug first: `vendor_format` preferred glb whenever the endpoint offered
  it, so `[fbx]` would have returned a glb and the completion guard would have
  failed a generation Rodin was perfectly capable of fulfilling — *after* it was
  billed. The FBX that came back carries the genuine
  `Kaydara FBX Binary \x00` header, passed through untouched.
- **`Model3dAssetKind::Texture` is produced for the first time.** The kind has
  been declared and consumed since 3D shipped with nothing ever filling it;
  Rodin's `textures` array fills it, and the `(stem, format)` rehost keying
  names them `texture.png`, `texture_1.png`, … rather than colliding.

**Run C** also confirmed fal's documented **2-concurrent limit**: it sat in
`IN_QUEUE` for ~90 s while A and B ran, surfaced correctly as `pending`.
Note its quote is the $0.16 base — fal charges 3× for a textured mesh, which is
recorded in the catalog's `variable_pricing` and deliberately not applied to the
estimate, since this endpoint cannot prove the multiplier.

Rejections, all at $0 and before any vendor call: `usdz` on a model that neither
emits nor derives it (`param_enum_mismatch`, listing the deliverable set);
`[glb, fbx]` on Rodin (`param_too_many` — 2 vendor jobs against `max_items: 1`);
a seed above Rodin's u16 ceiling; `symmetry` in strict mode; a text-only request
to an image-only model; an explicitly empty format list.

Still unproven: the 422 and `error_type` paths, which need a request fal itself
rejects rather than one we reject first.

### Superseded: the first single-run proof, `fal/trellis`

The first end-to-end evidence that any of this works against a vendor rather
than a mock. `fal/trellis` is **image-to-3D and emits GLB only** — the exact case
the design was built for.

Request: `output_formats: [glb, obj, stl, ply]` against fal's own example image.
Quoted $0.02, charged $0.02, terminal in ~20s (`IN_QUEUE` → `IN_PROGRESS` →
`COMPLETED`). Result — four containers from a one-container vendor:

| format | bytes | source | content-type served |
|---|---|---|---|
| glb | 1,819,360 | fal, verbatim | `model/gltf-binary` |
| obj | 1,325,575 | derived locally | `model/obj` |
| stl | 829,584 | derived locally | `model/stl` |
| ply | 421,179 | derived locally | `application/octet-stream` |

Structurally validated after download, not merely non-empty:

- **16,590 triangles and 12,824 vertices, identical across obj, stl and ply.**
- glb: `glTF` magic, version 2, declared length == actual length.
- obj: no `mtllib`/`usemtl` — the self-contained rule holds, so rehost renaming
  cannot break a sidecar reference.
- stl: binary, `84 + 50n` exact, and its header does **not** begin with `solid`
  — the guard against naive ASCII-detection elsewhere.
- ply: `binary_little_endian`, vertex and face counts matching the header.

`ply` is served as `application/octet-stream` deliberately: PLY has no IANA
registration, and inventing one would be worse — the same call made for `fbx`.

Still unproven by this run: Rodin and Hunyuan3D (not exercised), the `textures`
array (Rodin-only), and the 422/`error_type` paths, which need a deliberately
invalid request to trigger.

### `output_formats` is not like the other rows

Every other param is a hint the vendor may ignore; this one is a PROMISE —
whatever survives validation must be present on a `completed` generation or
the generation fails, enforced at all three observers of completion. Two
consequences for this matrix:

- `max_items` counts **vendor jobs, not deliverables**. litegen derives every
  container it can read locally (`litegen-core/src/mesh/`; since the
  2026-10-12 rewrite that is all ten — glb, gltf, obj, stl, ply, fbx, usdz, usda,
  3mf, dae — see `3d-format-conversion.md`), so any set of them costs one vendor
  job. Rodin's scalar field therefore serves `[glb, fbx]` from one job: its own
  fbx (`prefers_vendor_native`) with the glb derived from it. Before the
  rewrite fbx was underivable and that request was refused as two jobs. The
  API serves this to clients as `params.output_formats.derivable_values` beside
  the widened `enum_values`
  (`model3d_api::fal_3d_models_are_served_with_their_deliverable_formats_and_vendor_reference`).
- A promised container can still go missing when the vendor's own file is one
  the converter cannot read — a Draco-compressed glb, say — because nothing is
  then derivable from it. `mock/partial-3d` is exactly that case, and the
  completion guard fails it at every observer. A vendor's own fbx or usdz still
  matters, as the higher-fidelity export `prefers_vendor_native` asks for: a cell
  claiming one must be proven by a live call before it is ticked.

### Not yet implemented: first-party vendors

Design notes for the vendors the design spec names, transcribed from its §9
vendor tables. **None of these is implemented**, and no cell here has been
exercised against the vendor. Cells tagged `(unconfirmed)` do not appear in §9
at all — guesses to verify against vendor docs, not spec-sourced field names.

| litegen param | Meshy | Tripo3D | Stability | Rodin (direct) | Status |
|---|---|---|---|---|---|
| `prompt` | `prompt` | `prompt` | — (image only) | — | ⬜ |
| `output_formats` | `target_formats` (glb\|obj\|fbx\|stl\|usdz\|3mf; omitted ⇒ all but 3mf) | separate billed convert task; `quad` forces FBX | fixed GLB, no param | `geometry_file_format` (scalar) | ⬜ |
| `symmetry` ⚠️ | `symmetry_mode` — **deprecated by Meshy** | — | — | — | ⚠️ we are the only consumer of a param its one vendor deprecated |
| `texture` | `should_texture` | `texture` (unconfirmed) | — | `material` | ⬜ |
| `pbr` | (texture_* maps) | `pbr` | — | `material=PBR` | ⬜ |
| `target_polycount` | `target_polycount` | `face_limit` | `vertex_count` | `quality` | ⬜ |
| `topology` | `topology` | `quad` (bool) | `remesh` | `mesh_mode` | ⬜ |
| `rig` | (separate endpoint — unconfirmed) | `rig` (unconfirmed) | — | — | ⬜ |
| `seed` | — | `model_seed` | `seed` | — | ⬜ |
| ref role `init` | `image_url` (b64 ok) | `file_token`\|url | multipart `image` | multipart `images` | ⬜ |
| ref roles `view-*` | multi-image-to-3d | multiview-to-model | — | `condition_mode` | ⬜ |

### Contract coverage that is not vendor mapping

None of this belongs in the vendor tables, because none of it proves a vendor
field name — but it is what makes the family behave the same on every model:

- `litegen-core/tests/model3d_validation.rs` — the validator against synthetic
  schemas (the mechanics of every rule) AND against the shipped catalog: each
  canonical 3D param refused in strict and dropped-and-cleared in lax on a
  model that does not declare it, and honoured on one that does. This closes
  the Task 8/9 follow-up the previous revision of this list left open.
- `litegen-core/tests/model3d_conformance.rs` — the catalog-wide contract on the
  CURRENT spelling (`output_formats`): glb in every declared set, every
  container in the `Model3dFormat` vocabulary, defaults satisfiable within
  `max_items`, the served set a superset of the native one, a `docs_url` on
  every vendor row, no-prompt models never requiring one, image-only models
  requiring their image.
- `litegen-core/tests/model3d_api.rs` — the HTTP lifecycle end to end against
  the mock provider, including what `GET /v1/models/{id}` serves.
- `litegen-core/tests/multitenant_api.rs` — a job the client's poll completes
  lands in the app's own bucket, and a DB error on a poll never answers with
  another org's job.
- `providers/model3d/mock.rs` and `providers/model3d/glb.rs` — the mock's
  progress ramp, real GLB bytes, failure model and outward winding.
- `tests/live_model3d.rs` — one `#[ignore]`d, self-skipping billed run against
  `fal/trellis` (`FAL_KEY`), asserting fal still answers in the shapes the
  wiremock suite mocks.
