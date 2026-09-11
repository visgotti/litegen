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

No vendor 3D adapters exist yet (Meshy/Tripo3D/Stability/Rodin are
deliberately deferred — see plan amendment 2026-08-20). Every cell in the
vendor columns below is transcribed from the design spec §9 vendor tables and
has **never been exercised against a live API or even a wiremock stub of one**.
Cells tagged `(unconfirmed)` are the exception in the other direction: they do
**not** appear in §9 at all and were inferred when this table was written, so
treat them as guesses to verify against vendor docs, not as spec-sourced field
names.
Do not upgrade a cell to ✅ on documentation alone — that is exactly the
mistake this document exists to prevent.

| litegen param | ParamSpec | Validator rule | Meshy | Tripo3D | Stability | Rodin | Proof |
|---|---|---|---|---|---|---|---|
| `prompt` | PromptSpec | required, length | `prompt` | `prompt` | — (image only) | — | ⬜ |
| `output_format` | String enum | enum_values | `model_urls.<fmt>` (response-side) | `quad`→FBX else GLB | fixed GLB | `format` | ⬜ |
| `texture` | Bool | supported-or-drop | `should_texture` | `texture` (unconfirmed) | — | `material` | ⬜ |
| `pbr` | Bool | supported-or-drop | (texture_* maps) | `pbr` | — | `material=PBR` | ⬜ |
| `target_polycount` | Int 100–300000 | range | `target_polycount` | `face_limit` | `vertex_count` | `quality` | ⬜ |
| `symmetry` | String enum | enum off/auto/on | `symmetry_mode` | — | — | — | ⬜ |
| `topology` | String enum | enum triangle/quad | `topology` | `quad` (bool) | `remesh` | `mesh_mode` | ⬜ |
| `rig` | Bool | supported-or-drop | (separate endpoint — unconfirmed) | `rig` (unconfirmed) | — | — | ⬜ |
| `seed` | Seed | range | — | `model_seed` | `seed` | — | ⬜ |
| ref role `init` | RefInputSpec | role declared | `image_url` (b64 ok) | `file_token`\|url | multipart `image` | multipart `images` | ⬜ |
| ref roles `view-*` | RefInputSpec | role declared | multi-image-to-3d | multiview-to-model | — | `condition_mode` | ⬜ |

What today's tests DO prove about the 3D family — none of it belongs in the
vendor table above, because it is mock-provider coverage, not vendor-mapping
coverage:
- `litegen-core/tests/model3d_validation.rs` (Task 8) exercises `validate_model3d` against **synthetic `ModelSchema`s built in the test file — not the mock catalog (`mock.yaml`)**. Pass-through in strict mode is proven only in the weak sense that all 7 declared params SURVIVE validation undropped (`supported_params_pass_through_untouched` asserts `dropped` is empty and re-checks 3 of the 7 values on the returned request); nothing there asserts a param reaches a provider or goes out under any field name. Lax-drop is proven for 4 of them only (`target_polycount`, `pbr`, `rig`, `topology`) and strict-reject for `target_polycount` alone. The remaining coverage is value-level, not per-param: polycount range, `topology` enum mismatch, multiview ref roles, required prompt. The deferred Task 8/9 follow-up — drop/reject proven per param against the real catalog — is still **open**.
- `litegen-core/src/providers/model3d/mock.rs` (Task 5 — 5 tests: progress ramp, one-mesh terminal poll, per-prompt distinctness, the `fail` model, unknown job id) plus its GLB writer `litegen-core/src/providers/model3d/glb.rs` (6 more at HEAD, on the container/mesh JSON/determinism/triangle count plus the outward-winding test added with the 3D review fixes — 11 between the two files; an earlier revision of this line credited all of them to `mock.rs` and counted 5 here) and `litegen-core/tests/model3d_api.rs` (Task 10) exercise the full request→response→poll lifecycle end to end against the mock provider — proving the *litegen-internal* plumbing works, not that any vendor mapping is right.
- `litegen-core/tests/catalog_conformance.rs`'s new `model3d_*` tests (Task 19, below) assert the family's structural contract (mode-vs-ref-role coherence, enum vocabularies, no image/video-only params, sane polycount bounds) across every advertised `model3d` row.

None of that substitutes for a real vendor test. The vendor table stays all
⬜ until a real adapter lands and gets a wiremock test asserting the outbound
field name — the same bar Image/Video are held to above.
