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

## Catalog fields

Fields a client reads off `GET /v1/models` (`capabilities`) and
`GET /v1/models/{id}` to decide what a model can do. They map to no vendor
field, so the proof is the loader rule, the catalog-wide conformance check and
the wire shape both SDKs type.

| field | where | rule | proof | status |
|---|---|---|---|---|
| `task` | `ModelSchema.task`, `ModelCapabilities.task` (`ModelTask`) | One per model, from its modality's list (3D: `generate` \| `rig` \| `animate` \| `texture` \| `remesh` \| `segment`; image: `generate` \| `multiview` \| `background_swap`; video: `generate` \| `extend` \| `retake` \| `video_edit`). Omitted in yaml means `generate`. The `*_to_3d` flags are refused on anything but a 3D `generate` model. | loader: `task_defaults_to_generate`, `loads_a_3d_operation_task`, `rejects_a_task_from_another_modality`, `rejects_to_3d_flags_on_a_non_generate_task`, `rejects_an_unknown_task`. conformance: `every_model_declares_a_task_its_modality_has`. wire: `model_task_is_a_named_enum_on_the_schema_and_the_capabilities`, `the_listing_carries_each_models_task`. SDKs: `test_literal_values_match_the_schema_enum[ModelTask]` (Python), `names every catalog task` (TS type test). | ✅ |
| `native_orientation` | `ModelSchema.native_orientation` (`NativeOrientation {up, front, evidence}`) | 3D only. The axes the vendor builds along, each one of `+x` `-x` `+y` `-y` `+z` `-z` spelled exactly so, at right angles, with the evidence that showed them. Absent means unproven: the mesh is never turned and its manifest says `front: "unverified"`. Declared today only by `mock/rig-3d` (+y/+z) and `mock/turned-3d` (+y/+x); every vendor row stays unproven until a live run shows its axes (no vendor keys here). | loader: `a_3d_model_declares_its_proven_orientation`, `native_orientation_axes_are_signed_axes_at_right_angles`, `native_orientation_names_its_evidence`, `native_orientation_is_a_3d_field`. wire: `a_3d_asset_documents_its_manifest_and_the_original_kind`, `the_catalog_says_which_models_have_a_proven_orientation`. SDKs: TS `types a mesh's manifest and a model's proven orientation`. | ✅ (mock); vendor rows 🟡 unproven |
| `is_available` | `ModelInfo.is_available` on `GET /v1/models` | True when the model's provider can serve the caller: their org stored its own credential for it, or litegen holds a platform default for the model's medium — the test a generation makes before answering `provider_not_configured` (L14). | HTTP `a_model_is_available_only_where_its_provider_can_serve_the_caller` (an org's own key flips its provider's models, and only those). | ✅ |
| `status`, `deprecated_at`, `retires_at`, `replaced_by` | `ModelSchema` (yaml dates, `YYYY-MM-DD`), `ModelInfo` | `status` is derived: `retired` strictly after `retires_at` (the vendor still serves it that day), `deprecated` once dated, else `active`. A retired model is neither listed nor resolved for a request; `replaced_by` names a catalog model that itself does not retire; a scheduled shutdown is its row's own `retires_at` (L14). | registry `a_model_past_its_retirement_is_no_longer_served`; loader `lifecycle_dates_are_calendar_days_and_a_model_is_not_its_own_replacement`; HTTP `the_listing_says_each_models_lifecycle`; conformance `every_scheduled_shutdown_is_its_rows_retires_at`, `every_replacement_is_a_catalog_model`; OpenAPI `the_listing_documents_lifecycle_limits_and_its_version` | ✅ |
| `max_reference_images` (`max_images` kept) | `ModelCapabilities` | The reference limit (`ref_inputs.max_total`); `max_images` carries the same number for old clients (L14). | HTTP `the_listing_reports_real_limits_and_a_catalog_version` | ✅ |
| `max_duration_seconds` | `ModelCapabilities` | The `duration_seconds` param's max (float or int); none where the model has no duration param. | same test | ✅ |
| `catalog_version` | `ModelListResponse` | The first 16 hex of SHA-256 over every served model (sorted by id): changes whenever any served field does. | registry `the_catalog_version_changes_with_any_field`; same HTTP test | ✅ |

## Request-time checks (L13)

What a model cannot do is refused before anything is billed, with a code that
says so, instead of failing (or being swapped) at the vendor.

| check | rule | proof | status |
|---|---|---|---|
| capability | 400 `capability_unsupported`: text alone on an image model without `text_to_image` or a video model without `text_to_video`; an `init` image on an image model with neither `image_to_image` nor `inpainting`; a frame (`first_frame`, `last_frame`, `init`, `keyframe`) on a video model without `image_to_video`; no reference on a 3D model without `text_to_3d` (a required `init` role still answers `ref_role_required` first) | validator `a_capability_the_model_lacks_is_refused_by_name` | ✅ |
| `n` | litegen returns one image per request, and a vendor asked for more bills them all, so `n > 1` is 400 `param_out_of_range` (param `n`) | validator `more_than_one_image_per_request_is_refused` | ✅ |
| model ids | an id an adapter has no arm for is `ModelNotSupported`, never billed as another model: Runway (image, video), Leonardo (image, video), Luma (image, video), fal (image, video — `fal/video` is now an arm of its own) and Google image; Google video passes its id through | each adapter's `every_catalog_row_resolves_and_an_unknown_id_does_not` (every row has its own arm) | ✅ |
| sizes | refused, never snapped: a freeform size spec now carries `min_pixels`, `max_pixels` and `max_aspect`, and the GPT Image 2 rows declare OpenAI's (655,360–8,294,400 pixels, 3:1); the OpenAI adapter refuses anything else instead of snapping it | validator `a_size_the_vendor_cannot_make_is_refused_not_snapped`; adapter `gpt_image_2_sizes_follow_the_documented_constraints`; conformance `gpt_image_sizes_carry_every_openai_limit`; OpenAPI `a_freeform_size_documents_the_vendors_pixel_and_aspect_limits` | ✅ |

## Vendor errors (L15)

Once every attempt has failed, the adapter's last error decides what the
caller sees; every error body's `code` is a string (the error's `type` unless
a more specific code exists).

| vendor answer | litegen | body | proof | status |
|---|---|---|---|---|
| 4xx (but 401/403, 408, 429) | 400 `vendor_rejected`, `retryable: false` | `provider_error: {status, code, message}` read from the vendor's error shape (OpenAI `error.{code,message}`, fal `detail[0].{type,msg}`, `{code, message}` envelopes) | HTTP `a_vendors_4xx_is_a_litegen_4xx_that_names_the_vendors_reason` | ✅ wiremock |
| 401 / 403 (litegen's key refused) | 424 `vendor_rejected` | same | — | 🟡 |
| 5xx or no answer | 502 `vendor_error`, `retryable: true` | same | HTTP `a_vendors_5xx_is_a_502_that_says_to_retry` | ✅ wiremock |
| 429 | 429 `vendor_error`, `retryable: true` | same | — | 🟡 |
| timeout | 504 `vendor_timeout`, `retryable: true`, `vendor_job_id` when accepted | — | existing timeout tests | ✅ |
| refused before any call (`InvalidRequest`, `ModelNotSupported`) | 400 | message | existing adapter tests | ✅ |
| every `code` | a string | — | HTTP `every_error_code_is_a_string`; SDKs `test_a_vendor_rejection_carries_the_vendors_answer_and_whether_to_retry`, `carries the vendor's own answer and whether to retry`; OpenAPI `an_error_documents_the_vendors_own_answer_and_whether_to_retry` | ✅ |

## Prices that match the bill (3D doc §3G, L14)

A row's `pricing.base_cost_usd` is the vendor's price for its default request;
its `pricing.variable_pricing` rules follow the request from there. Rules
apply in order to a running price; a rule applies when every `when` entry
matches the request's facts — each param as sent or its default,
`extra.<field>` for allowlisted vendor fields, and the derived `views`,
`keyframes` and `animations` counts, `duration_seconds` (a keyframe model
without a duration param lasts until its last keyframe) and `megapixels`
(width × height ÷ 10⁶ of `size`, 1 when none is sent). Its one effect is a
price (`usd`), a rate (`per_second_usd`, `per_megapixel_usd`, `per_item_usd` ×
the `per_item` count) or an adjustment (`multiply`, `add_usd`).
`capabilities::pricing::request_price_usd` prices the `/cost` quote and the
quota reservation (× an image model's `n`), and the router charges the same
unit price per delivered output, so the three agree.

| what | rows | rule | proof | status |
|---|---|---|---|---|
| one price for quote, reservation and charge | every row | `request_price_usd` / `unit_price_usd` | HTTP `an_image_is_priced_by_its_megapixels_and_its_quality`, `a_video_is_priced_by_its_seconds_at_its_resolution`, `a_mesh_is_priced_by_its_multipliers_and_add_ons` (quote = charge); `a_reservation_holds_the_requests_own_price` (402 at the request's price, not the base); unit `capabilities::pricing::tests::*` | ✅ mock |
| the base is the default request's price | every priced row | conformance `every_base_price_is_what_the_rules_charge_for_the_default_request` | ✅ |
| rules are well formed | every row | one effect each; `per_item` with `per_item_usd`; `when` names a param, an allowlisted `extra.<field>` or a derived fact; `per_second_usd` on video, `per_megapixel_usd` on images; nothing negative but `add_usd` | loader `a_pricing_rule_names_one_effect_over_facts_the_request_has` | ✅ |
| per megapixel | `fal/flux-pro` 0.04, `fal/flux-dev` 0.025, `fal/flux-schnell` 0.003, `fal/flux-2` 0.012, `fal/sd35-medium` 0.02 | `per_megapixel_usd` | fal's "per megapixels" (llms.txt); how fal counts or rounds megapixels is undocumented **(unverified)** | 🟡 |
| per second | `runway/gen4.5` 0.12, `runway/gen4-turbo` 0.05; `kling/video-kling-v2-5-turbo`, `-v2-6`, `-v3` by `extra.mode` / `extra.sound`; `minimax/MiniMax-H3`, `-H3-Max` by `resolution`; `vidu/viduq3-turbo` 0.055, `viduq3-pro` 0.10 (720p; other resolutions unpublished in the rows), `viduq2-pro-fast` $0.04 + $0.01/s after the first; `fal/ltx-2.3` by `resolution`; `pixverse/v6`, `pixverse/c1` by `resolution` and `extra.generate_audio_switch` | `per_second_usd` (+ `add_usd`) | each row's quoted vendor rate | 🟡 |
| fixed tiers | `pixverse/v5` (5 s / 8 s × resolution), `pixverse/v5-multi-transition` (1–10, 15, 20, 25, 30 s × resolution), `ideogram/ideogram-4-5-precise-edit` (quality) | `usd` by value | rows' quoted tables | 🟡 |
| multipliers and add-ons | `fal/hyper3d-rodin` HighPack ×3, `fal/hunyuan3d-v2` textured ×3, `fal/hunyuan3d-v3.1-pro` +$0.15 each for PBR, views, a face count, `fal/tripo-h3.1-multiview` $0.20 untextured | `multiply`, `add_usd`, `usd` | fal llms.txt quotes | 🟡 |
| per clip | `tripo/animate` $0.10, `meshy/animate` $0.06 | `per_item_usd` × `animations` | Tripo pricing; Meshy provisional | 🟡 |

PixVerse states "225 Credits = USD 1" (2026-10-30); its rows were priced at
$0.01 a credit from a page that is now gone, so every PixVerse price fell to
that reference. Not converted, for want of a re-readable formula: BFL's FLUX.2
rows (its price page renders the numbers client-side), MiniMax Hailuo's
per-clip tiers and Vidu's other resolutions.

## Request headers

Headers every billed POST (`/v1/images/generations`, `/v1/videos/generations`,
`/v1/models3d/generations`) honours, whatever the model. They reach no vendor.

| header | rule | proof | status |
|---|---|---|---|
| `Idempotency-Key` | 1–255 visible ASCII characters, scoped to the caller's org, app and key. Same key + same request (method, path, canonical body) within 24 h: the stored response, byte for byte, with `Idempotent-Replayed: true`, and nothing runs or bills. Same key + another request: 409 `idempotency_key_reused`. Same request still running: 409 `idempotency_request_in_progress` with `Retry-After`. A request refused before any vendor call (validation, auth, quota, unconfigured provider) does not spend the key. A record left `in_progress` for 15 min is abandoned. | HTTP: `tests/idempotency_api.rs` (all three families replay with one vendor job and one log row; conflict; unspent key on a validation error; malformed key; a timed-out call replayed as its 504). storage: `a_finished_request_replays_and_a_new_body_conflicts`, `a_released_key_can_run_again`, `a_record_older_than_a_day_is_forgotten_and_a_stale_run_abandoned`, `racing_first_requests_start_exactly_once`, Postgres `idempotency_records_against_postgres`. hashing: `a_json_body_hashes_the_same_whatever_its_layout`, `a_multipart_body_hashes_the_same_whatever_its_boundary`, `keys_belong_to_their_owner`. wire: `every_billed_post_documents_its_idempotency_key`. SDKs: `idempotencyKey` (TS `Idempotency-Key` tests), `idempotency_key` (Python, sync and async). | ✅ |

**Vendor calls are never re-sent once they may have been billed** (L1). A call
that ran past `server.vendor_timeout_secs` (default 120) answers 504
`vendor_timeout` with the vendor's job id when it had accepted one, and is
neither retried nor handed to the next deployment; a failure after an adapter
recorded the vendor's job id is not retried either. Proof:
`a_timed_out_attempt_is_never_sent_again`,
`a_deployment_that_timed_out_does_not_fall_through_to_the_next`,
`a_job_the_vendor_accepted_is_never_sent_again`, and
`a_timed_out_vendor_call_is_never_resent` over HTTP with `mock/slow-image`
(one vendor call). Every image adapter that polls records the job id the moment
the vendor accepts it — bfl, kling, hunyuan, leonardo, luma, replicate, runway —
each asserted in its own polling test.

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

### Output format and transparency (L9)

`output_format` (`png` | `webp` | `jpeg`) and `background` (`transparent` |
`opaque`) are request fields, validated against each row's params (strict
refuses an undeclared one, lax drops and reports it); a transparent
background with a JPEG container is 400 `param_conflict`. The listing serves
`output_formats` (the row's `output_format` values), `supports_transparent_background`
and `lossy_only` (no PNG on offer). Every returned image's `content_type` is
read from its bytes (PNG, JPEG, WebP, GIF), whatever the adapter labelled it.

| rows | `output_format` | default litegen sends | `background` | wire | proof | status |
|---|---|---|---|---|---|---|
| `openai/gpt-image-2`, `-2.5-sunburst`, `-2.5-flare` | png, webp, jpeg | none (OpenAI's png) | transparent, opaque | `output_format`, `background` (JSON and multipart) | `generates_dalle3_image_b64_json` | 🟡 wiremock |
| `stability/{sd3-large, sd3.5-medium, sd3-turbo, core, ultra}` | png, webp, jpeg | png | — | multipart `output_format` | `generates_sd3_large_via_v2_multipart` | 🟡 wiremock |
| `bfl/*` (8) | png, webp, jpeg | png (BFL's own is jpeg) | — | `output_format` | `generates_flux_pro_via_polling_url` | 🟡 wiremock |
| `fal/flux-2` | png, webp, jpeg | none (fal's png) | — | `output_format` | — | 🟡 |
| `fal/flux-pro`, `flux-dev`, `flux-schnell`, `sd35-medium` | png, jpeg | none (fal's jpeg) | — | `output_format` | `generates_flux_dev_image` | 🟡 wiremock |
| `replicate/{flux-pro, flux-1.1-pro, flux-dev, flux-schnell, sd3}` | png, webp, jpeg | none (Replicate's webp) | — | `output_format`, jpeg sent as `jpg` | `generates_flux_dev_via_polling` | 🟡 wiremock |
| `luma/photon-1`, `photon-flash-1` | jpeg (`lossy_only`) | — (no format field) | — | — | HTTP `the_listing_says_what_each_model_can_return` | ✅ |
| `mock/format-image` | png, webp, jpeg | png | transparent, opaque | encodes as asked, labels everything png | HTTP `the_asked_format_comes_back_labelled_by_its_bytes`, `a_transparent_background_has_real_alpha`, `formats_and_backgrounds_a_model_cannot_make_are_refused` | ✅ mock |

Other image rows declare no `output_format`: their `output_formats` is empty
(the vendor decides) and `content_type` still comes from the bytes.

### References for URL-only vendors (L4)

Every model takes a base64 or uploaded reference. For a vendor that takes
references as URLs, the materializer sends the bytes inline as a `data:` URI
when the vendor accepts one (`ref_inputs.data_uri`, default true) and otherwise
publishes them at `{server.public_base_url}/v1/refs/{expiry}-{random}.{ext}`:
32 random bytes, answering for an hour, kept past the submit (an async vendor
fetches later), swept from local storage within the next hour after expiry.
When litegen's public origin is loopback, private or single-label (a laptop),
such a request is refused 400 before anything is billed.

| vendor rows | `data_uri` | evidence | proof | status |
|---|---|---|---|---|
| `luma/*` (5) | false | "You should upload and use your own cdn image urls, currently this is the only way to pass an image" (docs.lumalabs.ai/docs/image-generation.md, 2026-10-30) | conformance `only_vendors_that_refuse_data_uris_get_published_references` | 🟡 unverified live |
| `fal/*`, `runway/*`, `vidu/*`, `replicate/*` | true | fal: file inputs take data URIs (fal-cdn docs); Runway: "Data URIs are accepted in any field where a URL is accepted" (≤ 5 MB, jpeg/png/webp); Vidu: base64 images; Replicate: "HTTP URLs or data URLs" (≤ 256 KB recommended) | HTTP `fal_trellis_still_receives_the_upload_inline_as_a_data_uri` | ✅ wiremock |
| `mock/url-only-image` | false | — | HTTP `a_url_only_vendor_fetches_the_upload_from_an_unguessable_litegen_url`, `with_no_public_address_a_url_only_vendor_is_refused_before_billing`; materializer `a_base64_ref_for_a_url_only_vendor_is_published_at_an_unguessable_url`; publisher `a_published_ref_expires_within_the_hour`, `an_expired_or_unknown_token_is_not_served`, `the_sweep_removes_expired_refs_and_keeps_live_ones` | ✅ mock |

### Partial image edits (3D doc §4)

A caller sends ONE mask convention — an opaque image the size of `init`,
white (128 or more) edits, black keeps — and an optional `region` box in init
pixels (top-left origin, ends exclusive). Before anything is reserved,
`proxy::image_edit::prepare` reads the init image and the mask, refuses what
cannot be done, intersects mask and region, grows the area by `mask_grow_px`
where the vendor cannot, and hands the vendor its own form; the handler
replaces (or, for a region drawn as a mask, adds) the `mask` reference. Each
mask role names its vendor's convention (`roles.mask.convention`, P-R9); the
listing serves `mask_convention: white_edits` for every model that takes one.

| behaviour | rule | `openai/gpt-image-*` | `stability/inpaint` | `stability/sdxl` | `ideogram/ideogram-4-5-precise-edit` | mocks | proof | status |
|---|---|---|---|---|---|---|---|---|
| mask convention | caller white-edits → vendor's own | `transparent_edits` (alpha 0 where white) | `white_edits` | `white_edits` (`mask_source=MASK_IMAGE_WHITE`) | `black_edits` | inpainting `white_edits`, region `black_edits` | core `a_white_edits_mask_becomes_each_vendors_convention`; HTTP `openai_receives_the_mask_transparent_where_the_caller_painted_white`, `stability_receives_the_white_mask_as_sent_and_an_explicit_grow_mask`; conformance `ideogram_precise_edit_takes_a_black_mask_and_a_256_px_box`, `stability_inpaint_takes_a_white_mask_and_grows_it_itself` | 🟡 wiremock / ✅ mock |
| `region` | a box-native vendor gets its own box; a mask-only one the box drawn as the mask; with a mask, where they overlap | drawn as the mask | drawn as the mask | — (no region) | `context_window` = `y,x,y+h,x+w`; a region under 256 px a side gets a 256 px window (1:6–6:1, kept on the image) and its exact area as the mask | inpainting: drawn; region: box, 256 px minimum | core `a_small_region_gets_the_vendors_minimum_window_and_its_exact_area_as_the_mask`, `a_window_is_kept_on_the_image_and_within_the_vendors_aspect_ratio`, `a_region_and_a_mask_edit_only_where_they_overlap`; HTTP `a_region_reaches_ideogram_as_its_exact_context_window`, `a_small_region_reaches_ideogram_as_a_256_window_and_its_exact_area_in_black`, `a_region_on_a_mask_model_arrives_drawn_as_its_mask_and_overlaps_a_mask`, `a_small_region_on_a_box_model_gets_its_minimum_window_and_its_exact_area` | 🟡 wiremock / ✅ mock |
| `mask_grow_px` | the vendor's own growth where it has one, sent explicitly (even 0); else litegen dilates (square, both axes); ≤ 100 | litegen dilates | `grow_mask` (default 5 overridden) | litegen dilates | litegen dilates | litegen dilates | core `litegen_grows_the_edit_area_where_the_vendor_cannot`, `a_vendor_that_grows_masks_gets_the_exact_mask_and_an_explicit_growth`; HTTP `stability_grows_its_own_mask_when_asked`, `mask_grow_px_grows_the_edit_area` | 🟡 wiremock / ✅ mock |
| refusals, before billing | `init_required` (400, validator), `mask_size_mismatch`, `mask_ambiguous` (any alpha < 255), `region_out_of_bounds` (400), `region_too_small` (422: no window fits, or a box vendor takes no mask), `image_unreadable` (400) | ✓ | ✓ | ✓ | ✓ | ✓ | HTTP `a_bad_edit_is_refused_before_anything_is_billed` (no row, no log); core tests for each code | ✅ |
| untouched pixels | `preserves_unmasked`: the vendor's own words | — (`unspecified`… not declared) | `unspecified` | — | `exact` ("copied exactly") | `exact`: every other pixel identical | HTTP `the_inpainting_mock_fills_exactly_the_masked_area` (every pixel compared) | ✅ mock |
| listing | `supports_inpainting` (takes a mask), `supports_region_edit`, `region_input`, `preserves_unmasked`, `mask_convention` | | | | | | HTTP `the_listing_says_how_each_model_edits_part_of_an_image`; conformance `the_inpainting_flag_means_the_model_takes_a_mask` | ✅ |

Not verified live: no OpenAI, Stability or Ideogram key is available here.

### Multiview images (3D doc §3E)

An image model with `task: multiview` turns its one `init` image into one
image per entry of `views` (the request's, in its order, or the row's
`default`), each a `data[]` entry carrying `view` and `yaw_deg`. Views come
from one vocabulary, `ImageView`, named for the side of the subject the camera
sees; `yaw_deg` is the camera's position around +Y from the subject's front
(+Z) towards +X: front 0, front-left 45, left 90, back-left 135, back 180,
back-right 225, right 270, front-right 315 (top and bottom have none). The
call is billed once, however many views come back.

| behaviour | rule | `tripo/multiview` | `mock/multiview-image` | proof | status |
|---|---|---|---|---|---|
| request | `init` (required) + `views` ⊆ the row's `enum_values`, unique, ≥ 1; no prompt where `max_length: 0` | `POST /v3/files` upload → `input: file_token` on `POST /v3/generation/image-to-multiview` (the only field); PNG, JPEG or WebP, anything else refused before any call | `init` + `views` from all eight yaw views | adapter `one_upload_one_task_and_one_image_per_asked_view_in_order`, `an_image_tripo_cannot_read_is_refused_before_any_call`; HTTP `views_the_model_does_not_make_or_repeat_are_refused`, `omitted_views_are_the_models_default_set` | 🟡 wiremock / ✅ mock |
| response | one image per view, in the asked order, `view` + `yaw_deg` | `output.generate_multiview_image.{front,left,back,right}_view_url`, polled every 2 s within 100 s, downloaded with no key | one tinted image per view | adapter test above; HTTP `one_image_comes_back_per_view_named_and_placed_by_yaw`; OpenAPI `multiview_documents_its_views_and_the_view_each_image_shows` | 🟡 wiremock / ✅ mock |
| failure | a failed or expired task is the vendor's reason, never resubmitted | `failed`/`cancelled`/`banned`/`expired` → non-retryable error | — | adapter `a_failed_task_reports_tripos_reason_and_is_never_resent` | 🟡 wiremock |
| billing | once per call | $0.10 (10 credits) | $0.01 | HTTP `a_multiview_call_is_billed_once_whatever_the_number_of_views` | ✅ mock |

Which side Tripo calls "left" is not documented **(unverified)**; litegen
names Tripo's `left_view_url` its `left` (yaw 90). Not verified live: no Tripo
key is available here.

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

### Timed keyframes (3D doc §5)

A `keyframe` reference carries `time_s`, seconds from the start of the clip;
`first_frame` and `last_frame` stay, as names for 0 s and the clip's end. The
row's `keyframe` role says how times are placed (`timing: any | even`, and a
`time_step_s` grid where the vendor takes only multiples of it); the listing
serves `supports_keyframes`, `max_keyframes` and `keyframe_timing`. Before
anything is reserved the validator refuses `time_s` on another role
(`param_unsupported`), a keyframe without one (`keyframe_time_required`), one
outside the clip or two at one time (`keyframe_time_out_of_range`), one off the
grid (`keyframe_off_grid`) and uneven times on an `even` model
(`keyframe_timing_uneven`; left out, litegen spaces them), and hands the
keyframes on in time order.

| behaviour | `pixverse/v5-multi-transition` | `mock/keyframe-video` | proof | status |
|---|---|---|---|---|
| times → vendor form | `multi_transition` items in time order, each `duration` the whole seconds to the next keyframe, the last 0, every segment carrying the prompt; the first keyframe must sit at 0 s (refused before any call) | frame `round(time_s × fps)` is the keyframe's image, unchanged; blends between | adapter `timed_keyframes_reach_multi_transition_as_segment_durations`, `a_multi_transition_starts_at_its_first_keyframe`; HTTP `the_frame_at_each_keyframes_time_is_that_keyframes_image` | 🟡 wiremock / ✅ mock |
| catalog | 2–31 keyframes, `timing: any`, `time_step_s: 1` | 0–8, `timing: any` | HTTP `the_listing_says_which_models_take_timed_keyframes`; loader `a_keyframe_role_says_how_its_times_are_placed`; OpenAPI `timed_keyframes_document_their_time_and_each_models_timing` | ✅ |
| refusals, before billing | the codes above | the codes above | HTTP `keyframe_times_are_checked_before_anything_is_billed`; validator `keyframes_reach_the_vendor_in_time_order`, `even_keyframes_are_spaced_by_litegen_or_must_already_be_even`, `a_keyframe_off_the_vendors_grid_is_refused` | ✅ |

Not carried yet: Luma `ray-3.2` (frame indexes on a 24 fps grid, up to 64)
lives on Luma's separate Agents API, and Runway `aleph2` takes keyframes only
on video-to-video, which needs `source_video` (P2). Not verified live: no
PixVerse key is available here.

## 3D (`model3d`)

**fal is the implemented 3D vendor** — `fal/hyper3d-rodin`, `fal/trellis` and
`fal/hunyuan3d-v2`, mapped by `litegen-core/src/providers/model3d/fal.rs`
(its multiview rows, `fal/tripo-h3.1-multiview` and `fal/hunyuan3d-v3.1-pro`,
are under [Multiview-to-3D](#multiview-to-3d-3d-doc-3e)).
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

### 3D operations (3D doc §2 rule 1, §3A)

An operation is a model whose `task` is not `generate`; it works on the mesh
named by `source_model`. The catalog declares what its vendor takes
(`params.source_model`: `formats`, `max_bytes`, `max_faces`, `requires_skin`,
`requires_texture`), and litegen holds the source to it before the billed
submit.

| litegen param | ParamSpec | rule | `mock/rig-3d` | proof | status |
|---|---|---|---|---|---|
| `source_model` | `source_model` | `{type: generation \| url \| base64, value, format?}`; `format` required for url/base64 (`source_format_required`), from the `Model3dFormat` vocabulary (`source_format_unsupported`). Required by every operation (`source_model_required`); refused (strict) / dropped (lax) elsewhere. A generation is read back after the `GET /v1/generations/{id}` access check: another org's or a mesh-less one is 404 `source_not_found`, an unfinished one 409 `source_not_ready`. Read with litegen's own reader; 422 `source_too_many_faces`, `source_too_large`, `source_not_rigged`, `source_not_textured`, `source_format_unsupported` before anything is reserved. Converted to the first accepted container when the source is in another. | glb only, ≤ 50 MiB, ≤ 20,000 triangles; chains a mock generation by the mock's own task id (`delivery: vendor_task`), else inline (`data_uri`) | loader: `a_3d_operation_declares_what_source_it_takes`, `a_3d_operation_without_a_source_model_is_refused`, `source_model_formats_come_from_the_mesh_vocabulary`. validator: `source_model_is_refused_in_strict_and_dropped_in_lax_where_undeclared`, `an_operation_without_its_source_is_refused`, `a_url_or_base64_source_needs_its_format`, `a_source_format_outside_the_vocabulary_is_refused`, `an_operation_needs_neither_a_prompt_nor_a_reference_image`. reader: `proxy::model3d_source::tests::*` (byte-for-byte pass-through, fbx → glb conversion with the received bytes' hash, every refusal, skinned and textured fixtures). HTTP: `tests/source_model_api.rs` (lineage `metadata.source = {generation_id, sha256, format, received_format, facts, delivery}`; another org's source 404; unfinished 409; a 20,001-triangle OBJ refused with no log or row; fbx base64 converted for a glb-only model). conformance: `every_3d_operation_declares_the_source_its_vendor_takes`. | ✅ (mock) |
| `reference_images[].type: "generation"` | `RefImageKind` | A stored image generation's id; read back after the same access check and sent as inline bytes, so every vendor form takes it. Another org's, a non-image, or a vanished one is 404 `source_not_found` (`param: reference_images`); unfinished 409. | — | `a_stored_image_is_a_reference_by_its_generation_id` (an image edit and image-to-3D from a stored image), `an_image_reference_another_org_owns_is_not_found`, `a_mesh_is_not_an_image_reference` | ✅ |
| row `vendor_account` | — | Every 3D row records the vendor account it ran on (`org:<id>` for an org credential, else `platform`); a source chains by the vendor's task id only on the same provider and account. | — | `a_rig_chained_by_generation_id_records_its_lineage` | ✅ |

### Rigging (3D doc §3B)

A `rig` model turns a mesh into a skinned one. Before the billed call litegen
runs the vendor's free preflight (`Model3dProvider::preflight`), after the
caller's limits and quota are checked: a refusal answers 422 with its code
(`not_riggable`) and settles the reservation to nothing.

| litegen param | ParamSpec | `tripo/rig` | `fal/meshy-rig` | `mock/rig-3d` | proof | status |
|---|---|---|---|---|---|---|
| `source_model` | `source_model` | `input`: the source's own Tripo task id when it is a Tripo job on the same account (`delivery: vendor_task`), else `POST /v3/files` multipart `file` → `file_token` (`upload`); glb, fbx, obj, stl ≤ 150 MB | `model_url` = the glb as a `data:model/gltf-binary;base64,…` URI (`data_uri`); glb ≤ 100 MB, ≤ 320,000 faces, textured (re-checked in the adapter) | any glb ≤ 20,000 triangles | tripo: `the_preflight_uploads_the_source_and_asks_rig_check`, `a_chained_source_goes_to_tripo_by_its_own_task_id`; fal: `meshy_rigging_sends_the_source_as_a_glb_data_uri_and_the_height`, `an_operation_without_its_source_is_refused_before_any_call`; conformance: `tripo_rig_takes_what_tripos_rig_endpoint_accepts`, `fal_meshy_rig_takes_a_textured_glb_within_meshys_limits` | 🟡 wiremock |
| `skeleton` | `string` (`Model3dSkeleton`) | `rig_type` (`humanoid` → `biped`, `other` → `others`); rig `model` `v3.0-20260909` for biped/others, `v2.5-20260210` for creatures; omitted → what rig-check found | — (humanoid only) | `humanoid` only | `a_humanoid_rig_asks_for_v3_and_the_requested_naming`, `vendor_naming_sends_no_spec_and_a_creature_asks_for_v2_5`, `an_omitted_skeleton_rigs_what_rig_check_found`; validator `a_skeleton_or_naming_the_rig_does_not_offer_is_refused`; loader `rejects_skeleton_and_joint_naming_outside_their_vocabularies` | 🟡 wiremock |
| `joint_naming` | `string` (`Model3dJointNaming`) | `spec` (`vrm`, `mixamo`, `unity`, `unreal`; `vendor` sends none); refused on a non-humanoid rig before any call | — | `vrm` emits VRM 1.0 bone names | `a_humanoid_rig_asks_for_v3_and_the_requested_naming`, `joint_naming_on_a_creature_is_refused_before_any_call`; HTTP `vrm_joint_naming_delivers_vrm_bone_names` | 🟡 wiremock / ✅ mock |
| `height_m` | `float` | — | `height_meters` (any positive value) | scales the humanoid (0.5–3 m) | `meshy_rigging_sends_the_source_as_a_glb_data_uri_and_the_height`, `meshy_rigging_without_a_height_leaves_merely_the_source`; HTTP `height_m_sets_the_rigged_characters_height`; validator `a_height_outside_the_rigs_range_is_refused` | 🟡 wiremock / ✅ mock |
| `output_formats` | `string_array` | `out_format`: `fbx` when fbx is promised, else `glb` | the endpoint returns both; glb is downloaded, Meshy's own fbx too when promised | glb | `an_fbx_promise_asks_tripo_for_its_own_fbx`, `a_finished_rig_downloads_the_promised_containers` | 🟡 wiremock |
| preflight | — | `POST /v3/animations/rig-check` (free), polled at `GET /v3/tasks/{id}`: `riggable: false`, a failed check, or a `rig_type` other than the requested skeleton → 422 `not_riggable` | none | a flat source (thinnest side < 1 % of longest) → 422 `not_riggable` | `a_source_rig_check_refuses_is_not_riggable`, `a_skeleton_rig_check_did_not_find_is_not_riggable`; HTTP `a_source_the_vendor_cannot_rig_is_refused_before_anything_is_billed` (no row, no log) | 🟡 wiremock / ✅ mock |
| manifest `skin` | — | measured from the delivered file | measured | `the_mock_rig_reports_its_skeleton_exactly` (§3I's joints and humanoid map) | `a_rig_no_table_knows_reports_the_requested_skeleton_and_no_humanoid_map` | ✅ |

Tripo statuses map `queued` → pending, `running` (and any unknown status) →
processing, `success` → completed after downloading `output.model_url` with no
key, and `failed`, `cancelled`, `banned`, `expired` → failed with Tripo's
`error_message` (`every_task_status_is_mapped`). Its error envelope
`{code, message, suggestion}` reaches the caller; a 429 or 5xx on submit is
retryable, a lost reply is not (`tripos_error_envelope_reaches_the_caller`).
Not verified live: no Tripo or fal key is available here.

### Animation (3D doc §3C)

An `animate` model adds one clip per `animations[]` entry to a rigged source,
in request order. litegen names the clips exactly as asked (dropping a static
pose clip a vendor adds, and failing the generation when fewer clips come back
than were asked), holds `in_place` clips the vendor did not, keys every clip at
`fps` when the vendor did not, and measures each clip into the manifest from
its keys. An animate model returns its source's geometry, so its catalog row
says `native_orientation: {up: source, front: source}` and the delivered front
is the source manifest's.

| litegen param | ParamSpec | `tripo/animate` | `meshy/animate` | `mock/animate-3d` | proof | status |
|---|---|---|---|---|---|---|
| `animations[].preset` | `animation_list` | `animations` (≤ 5; the 16 v2.5/v3.0 preset ids, 10 tagged canonical) | `action_ids` (≤ 10, no repeats — refused before the call; 678 library ids with Meshy's names, 12 tagged canonical) | `mock:*` (3 canonical) | validator `a_clip_names_a_canonical_preset_or_one_of_the_models_own`; tripo `a_retarget_sends_the_rigs_task_id_its_presets_and_tripos_in_place_flag`; meshy `an_animation_goes_to_the_meshy_rig_with_its_action_ids_and_change_fps`, `a_repeated_action_is_refused_before_any_call`; conformance `tripo_animate_offers_the_retarget_presets_on_tripos_own_rig`, `meshy_rig_and_animate_take_what_meshys_endpoints_accept` | 🟡 wiremock / ✅ mock |
| `animations[].name` | — | renamed by litegen in order | renamed by litegen | renamed by litegen | validator `clip_names_are_unique_and_well_formed`; delivery `a_pose_clip_beside_the_asked_ones_is_dropped_and_the_rest_named_in_order`, `a_vendor_returning_fewer_clips_than_asked_fails_the_delivery`; HTTP `one_clip_per_name_in_request_order_kept_in_place_by_the_vendor` | ✅ |
| `animations[].prompt`, `.duration_s` | `text_to_motion` | — | — (text-to-motion is a second billed task; not carried) | 1–10 s | validator `a_clip_is_exactly_one_of_a_preset_or_a_prompt`, `a_prompt_clip_lasts_within_the_models_text_to_motion_range`, `a_prompt_clip_needs_a_model_that_makes_motion_from_text`; HTTP `a_prompt_clip_lasts_the_duration_asked` | ✅ mock |
| `animations[].in_place` | `in_place: native \| litegen` | `animate_in_place: true` when every clip asks; otherwise litegen holds the in-place ones | litegen holds them (`in_place: litegen`) | natively when every clip asks | tripo `clips_that_differ_on_in_place_leave_it_to_litegen`; HTTP `a_travelling_clip_moves_one_metre_along_z_and_litegen_holds_the_in_place_one` (manifest `in_place_by`) | 🟡 wiremock / ✅ mock |
| `animations[].loop` | — | hint only | hint only | hint only | HTTP `one_clip_per_name_in_request_order_kept_in_place_by_the_vendor` (`loop_requested`) | ✅ |
| `fps` | `int` (3D: only beside `animations`) | — (litegen keys the clips) | `post_process: {operation_type: change_fps, fps}` at 24/25/30/60 (its fbx); litegen keys the glb | keys at `fps` | meshy `a_rate_meshy_cannot_make_is_left_to_litegen`, `a_finished_animation_downloads_the_glb_and_the_rate_changed_fbx_promised`; mesh `a_clip_is_rekeyed_at_the_asked_rate_and_left_alone_when_it_already_is`; HTTP `clips_are_keyed_at_the_requested_fps`; validator `fps_on_an_animation_stays_in_its_range` | 🟡 wiremock / ✅ mock |
| `source_model` | `source_model` | the rig's Tripo task id only; anything else → 422 `source_not_chainable` | `rig_task_id` only; anything else → 422 `source_not_chainable` | a rigged glb; unrigged → 422 `source_not_rigged` | tripo `a_source_tripo_did_not_rig_is_refused_before_any_call`; meshy `a_source_meshy_did_not_rig_is_not_chainable`; HTTP `an_unrigged_source_is_refused_before_anything_is_billed` | 🟡 wiremock / ✅ mock |

`meshy/rig` maps `source_model` to `model_url` (a glb `data:` URI) or
`input_task_id`, `skeleton` to `animation_type` (biped | quadruped) and
`height_m` to `height_meters` (`a_rig_sends_the_source_as_a_glb_data_uri_with_its_height_and_skeleton`,
`a_humanoid_rig_asks_for_a_biped_and_a_bare_one_sends_merely_the_source`).
Meshy statuses map PENDING → pending, IN_PROGRESS → processing, SUCCEEDED →
completed, FAILED and CANCELED → failed with `task_error.message`
(`every_task_status_is_mapped`); a create is never re-sent after a 5xx, as
Meshy asks (`meshys_message_reaches_the_caller_and_a_create_is_never_resent_after_a_5xx`).
Not verified live: no Tripo or Meshy key is available here.

### Normalised delivery (3D doc §2 rule 3, §3D)

One step, `proxy::model3d_deliver::deliver`, runs on a finished job's files at
whichever observer sees completion first (the client's poll or the poller),
from one plan fixed at submit (`metadata.delivery`). It maps no vendor field;
it is what every vendor's mesh goes through before litegen stores it.

| behaviour | rule | proof | status |
|---|---|---|---|
| read | The vendor's mesh is read with litegen's own reader. One it cannot read (Draco, meshopt) fails the generation, naming it and any promised container that cannot be derived (P-R7). | `a_mesh_litegen_cannot_read_fails_the_delivery_and_names_what_is_missing`; HTTP `a_mesh_litegen_cannot_read_is_never_delivered`; `a_provider_that_under_delivers_an_underivable_format_fails_the_generation` | ✅ |
| orientation | A model whose `native_orientation` differs from +Y up / +Z front gets one root rotation; the manifest says `oriented_by: "litegen"`. Unproven: never turned, `front: "unverified"`. | `a_turned_mesh_is_rotated_to_face_z_and_the_vendors_file_kept`, `an_unproven_orientation_is_reported_never_guessed`; mesh `a_mesh_facing_x_is_turned_to_face_z`, `a_z_up_mesh_is_stood_up`; HTTP `a_turned_vendor_mesh_is_delivered_facing_z_with_the_vendors_file_kept`, `a_model_whose_orientation_is_unproven_says_so` | ✅ (mock) |
| extensions | No delivered glTF requires an extension: WebP textures become PNG; a KTX2 texture is dropped from the normalised copy and logged (P-R8); a vendor glb that declares `extensionsRequired` is rewritten. | `a_webp_textured_mesh_needs_no_extension_once_delivered`, `webp_textures_become_png_and_nothing_needs_an_extension`; HTTP `every_asset_hash_matches_the_bytes_litegen_serves` (empty `extensionsRequired`) | ✅ |
| names | Node names are made unique (`arm`, `arm_1`), so a bone map is unambiguous. | `duplicate_node_names_get_numeric_suffixes`, `duplicate_joint_names_are_delivered_with_numeric_suffixes` | ✅ |
| `original` | When litegen changed the mesh, every vendor mesh is kept as an asset of kind `original`; when nothing changed, the vendor's bytes are delivered as they arrived. | `a_conforming_mesh_passes_through_with_a_measured_manifest` (bytes unchanged), `a_turned_mesh_is_rotated_to_face_z_and_the_vendors_file_kept` | ✅ |
| `scene` manifest | Every delivered mesh carries `scene`, measured from its own bytes: `units`, `up`, `front`, `oriented_by`, `bounds`, vertex / triangle / UV-set counts, `vertex_colors`, `materials[].maps`, `skin` (`joints`, `root`, `bind_pose` t/a/unknown, VRM 1.0 `humanoid` map from the VRM, Unity, Mixamo, Meshy or litegen-mock naming tables), `animations` (`duration_s`, measured `in_place` with `root_travel_m`, `in_place_by`), `extensions_used`. | `a_conforming_mesh_passes_through_with_a_measured_manifest` (the mock cube exactly: 8 vertices, 12 triangles, no UVs, one material, bounds ±its scale), `a_rig_reports_its_skin_and_humanoid_map`; `mesh::inspect::tests::*` (validated against fal's published Meshy rig and animation samples); `mesh::humanoid::tests::*` | ✅ |
| both observers | The client's poll and the poller deliver the same files byte for byte, with the same manifests. | `the_client_and_the_poller_deliver_identical_assets` (mutation-checked: removing the poller's delivery fails it) | ✅ |
| `sha256` | Every stored asset's `sha256` is the hash of the bytes litegen serves for it. | `every_asset_hash_matches_the_bytes_litegen_serves` | ✅ |

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

### Multiview-to-3D (3D doc §3E)

A 3D generate model's mode comes from its references: `view-<name>` roles
(any `ImageView`) mean multiview. Before anything is reserved the validator
refuses an `init` image beside views (`ref_role_conflict`), views without
`view-front` (`ref_role_required`), and — for a vendor that reads its views by
position, whose row declares `ref_inputs.view_order` — a view sent without
every view before it (`ref_role_required`, naming the missing view). The fal
adapter then places each view where its endpoint takes it.

| litegen | `tripo3d/h3.1/multiview-to-3d` (`fal/tripo-h3.1-multiview`) | `fal-ai/hunyuan-3d/v3.1/pro/image-to-3d` (`fal/hunyuan3d-v3.1-pro`) | proof | status |
|---|---|---|---|---|
| views | `image_urls`, 2–4 in the order `[front, left, back, right]` (`view_order`); front and left required | `input_image_url` (front), `back_`, `left_`, `right_`, `top_`, `bottom_`, `left_front_` (front-left), `right_front_image_url` (front-right) | adapter `tripo_multiview_receives_its_views_as_one_list_in_its_own_order`, `hunyuan_3_1_takes_each_view_in_its_own_field`; HTTP `views_reach_tripo_on_fal_in_tripos_order_whatever_order_they_were_sent_in`; validator `multiview_to_3d_needs_a_front_view`, `an_init_image_and_views_together_are_refused`, `views_a_vendor_takes_as_one_ordered_list_cannot_skip_one`; HTTP `views_a_multiview_model_cannot_take_are_refused_before_billing`; loader `a_view_order_names_only_views_the_model_takes_once_each`; tie `each_catalog_row_declares_exactly_what_its_endpoint_takes` | 🟡 wiremock |
| "left" | **(unverified)**: Tripo does not say whose left | the subject's left: Tencent's own multiview example (Hunyuan3D-2, `assets/example_mv_images/1`) shows in `left.png` the arm its front view puts on the subject's left | — | 🟡 docs / ⬜ live |
| `init` | — (multiview only; a lone image is refused) | `input_image_url` | `tripo_multiview_refuses_a_lone_image_before_submitting`, `hunyuan_3_1_takes_an_init_image_as_its_front_and_sends_no_add_on_unasked` | 🟡 wiremock |
| `texture` | `texture` (default true) | `generate_type`: `Normal` / `Geometry` | adapter tests above | 🟡 wiremock |
| `pbr` | `pbr` (default true) | `enable_pbr` (+$0.15) | adapter tests above | 🟡 wiremock |
| `target_polycount` | `face_limit`, 1000–2000000 | `face_count`, 40000–1500000 (+$0.15; sent only when asked) | adapter tests above; tie test (range) | 🟡 wiremock |
| `seed` | `model_seed` | — (no seed field) | `tripo_multiview_receives_its_views_as_one_list_in_its_own_order` | 🟡 wiremock |
| price | $0.30 textured (default); $0.20 untextured, recorded in `variable_pricing` | $0.375; +$0.15 each for views, PBR and a face count, recorded in `variable_pricing` | — (Task 17 applies `variable_pricing`) | 🟡 |

Not verified live: no fal key is available here, so neither vendor's "left"
has been proven with an asymmetric fixture yet.

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
| ref roles `view-*` | multi-image-to-3d | multiview-to-model (on fal: `fal/tripo-h3.1-multiview`, above) | — | `condition_mode` | ⬜ |

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
- `litegen-core/tests/model3d_chain.rs` — the 3D doc §3J chain end to end:
  a base64 still → mock/mesh-3d → mock/rig-3d by generation id →
  mock/animate-3d with an in-place `walk` and `idle`, litegen restarted over
  the same database file and asset directory between every step, and every
  step re-sent with its `Idempotency-Key` answering the same id and billing
  nothing. The final GLB is read by litegen and by `@gltf-transform/core`
  (`scripts/mesh-verify/read-gltf.mjs`; skipped, loudly, until `npm ci` runs
  there): exactly `walk` and `idle`, 1 s each, 7 joints, nothing required.
- `litegen-core/tests/animate_api.rs` — animation over HTTP against
  `mock/animate-3d`: clip order and names, in place by the vendor or litegen,
  `fps`, prompt length, the loop hint, the inherited front, and an unrigged
  source costing nothing.
- `litegen-core/tests/rig_api.rs` — rigging over HTTP against `mock/rig-3d`:
  the exact §3I skeleton and humanoid map, `joint_naming: vrm`, `height_m`,
  and a source the preflight refuses costing nothing.
- `litegen-core/tests/delivery_api.rs` — normalised delivery over HTTP: a
  turned vendor mesh delivered facing +Z with its `original`, every asset's
  hash matching the served bytes, an unproven model reporting
  `front: "unverified"`, the catalog's `native_orientation`, and an unreadable
  mesh failing even when glb alone was asked for.
- `litegen-core/tests/model3d_api.rs` — the HTTP lifecycle end to end against
  the mock provider, including what `GET /v1/models/{id}` serves, and that a
  stored mesh still downloads from a restarted process
  (`a_mesh_still_downloads_after_a_restart`).
- `litegen-core/tests/job_rows.rs` (L2) — a video or 3D submit answers only
  after its generation row is written, so its id answers on the very next
  request; a row that cannot be written fails the submit with 500
  `generation_record_failed` (naming the vendor's job, and cancelling a 3D
  one at the vendor); and the row's `metadata.request` records the request —
  prompt, params, `extra` and each reference as SHA-256 + size or URL, never
  its bytes.
- `litegen-core/tests/assets_api.rs` (L3) — litegen keeps every result: a
  finished video is re-hosted by whichever observer sees it finish (from the
  vendor's inline bytes, or an SSRF-checked download of its URL) and reads the
  same however often it is read; an image result gets a `completed` row and
  its inline images are stored; `GET /v1/generations/{id}/assets` lists every
  kept file with its size and SHA-256, and `…/assets/{index}` streams it with
  a litegen key alone, 404 to another org. The poller's half:
  `poll_once_keeps_a_finished_video_on_litegen_storage`.
- `proxy::storage::model3d_storage_tests` — the local backend keeps assets on
  disk (`a_store_over_the_same_dir_serves_what_an_earlier_one_stored`), refuses
  keys that would leave its directory, sweeps only what is past retention, and
  never retains for less than 7 days.
- `litegen-core/tests/multitenant_api.rs` — a job the client's poll completes
  lands in the app's own bucket, and a DB error on a poll never answers with
  another org's job.
- `providers/model3d/mock.rs` and `providers/model3d/glb.rs` — the mock's
  progress ramp, real GLB bytes, failure model and outward winding.
- `tests/live_model3d.rs` — one `#[ignore]`d, self-skipping billed run against
  `fal/trellis` (`FAL_KEY`), asserting fal still answers in the shapes the
  wiremock suite mocks.
