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
| `task` | `ModelSchema.task`, `ModelCapabilities.task` (`ModelTask`) | One per model, from its modality's list (3D: `generate` \| `rig` \| `animate` \| `texture` \| `remesh` \| `segment`; image: `generate` \| `multiview` \| `background_swap` \| `segment`; video: `generate` \| `extend` \| `retake` \| `video_edit`). Omitted in yaml means `generate`. The `*_to_3d` flags are refused on anything but a 3D `generate` model. | loader: `task_defaults_to_generate`, `loads_a_3d_operation_task`, `rejects_a_task_from_another_modality`, `rejects_to_3d_flags_on_a_non_generate_task`, `rejects_an_unknown_task`. conformance: `every_model_declares_a_task_its_modality_has`. wire: `model_task_is_a_named_enum_on_the_schema_and_the_capabilities`, `the_listing_carries_each_models_task`. SDKs: `test_literal_values_match_the_schema_enum[ModelTask]` (Python), `names every catalog task` (TS type test). | ✅ |
| `native_orientation` | `ModelSchema.native_orientation` (`NativeOrientation {up, front, evidence}`) | 3D only. The axes the vendor builds along, each one of `+x` `-x` `+y` `-y` `+z` `-z` spelled exactly so, at right angles, with the evidence that showed them. Absent means unproven: the mesh is never turned and its manifest says `front: "unverified"`. Declared today only by `mock/rig-3d` (+y/+z) and `mock/turned-3d` (+y/+x); every vendor row stays unproven until a live run shows its axes (no vendor keys here). | loader: `a_3d_model_declares_its_proven_orientation`, `native_orientation_axes_are_signed_axes_at_right_angles`, `native_orientation_names_its_evidence`, `native_orientation_is_a_3d_field`. wire: `a_3d_asset_documents_its_manifest_and_the_original_kind`, `the_catalog_says_which_models_have_a_proven_orientation`. SDKs: TS `types a mesh's manifest and a model's proven orientation`. | ✅ (mock); vendor rows 🟡 unproven |
| `is_available` | `ModelInfo.is_available` on `GET /v1/models` | True when the model's provider can serve the caller: their org stored its own credential for it, or litegen holds a platform default for the model's medium — the test a generation makes before answering `provider_not_configured` (L14). | HTTP `a_model_is_available_only_where_its_provider_can_serve_the_caller` (an org's own key flips its provider's models, and only those). | ✅ |
| `status`, `deprecated_at`, `retires_at`, `replaced_by` | `ModelSchema` (yaml dates, `YYYY-MM-DD`), `ModelInfo` | `status` is derived: `retired` strictly after `retires_at` (the vendor still serves it that day), `deprecated` once dated, else `active`. A retired model is neither listed nor resolved for a request; `replaced_by` names a catalog model that itself does not retire; a scheduled shutdown is its row's own `retires_at` (L14). | registry `a_model_past_its_retirement_is_no_longer_served`; loader `lifecycle_dates_are_calendar_days_and_a_model_is_not_its_own_replacement`; HTTP `the_listing_says_each_models_lifecycle`; conformance `every_scheduled_shutdown_is_its_rows_retires_at`, `every_replacement_is_a_catalog_model`; OpenAPI `the_listing_documents_lifecycle_limits_and_its_version` | ✅ |
| `max_reference_images` (`max_images` kept) | `ModelCapabilities` | The reference limit (`ref_inputs.max_total`); `max_images` carries the same number for old clients (L14). | HTTP `the_listing_reports_real_limits_and_a_catalog_version` | ✅ |
| `max_duration_seconds` | `ModelCapabilities` | The `duration_seconds` param's max (float or int); none where the model has no duration param. | same test | ✅ |
| `supports_segmentation`, `segmentation_prompts` | `ModelCapabilities` | True, and the row's `params.prompts.types` (`point` \| `box` \| `text`), exactly when the model is an image `segment` model with a `segmentation_prompts` spec — which the loader requires of those and refuses on any other (L7). | loader `a_segmentation_model_says_which_prompts_it_takes_and_makes_nothing_else`; HTTP `the_listing_says_which_models_segment_and_with_which_prompts`; SDKs TS `types prompts, masks and the catalog fields`, Python `test_literal_values_match_the_schema_enum[SegmentationPromptType]` | ✅ |
| `catalog_version` | `ModelListResponse` | The first 16 hex of SHA-256 over every served model (sorted by id): changes whenever any served field does. | registry `the_catalog_version_changes_with_any_field`; same HTTP test | ✅ |

## Request-time checks (L13)

What a model cannot do is refused before anything is billed, with a code that
says so, instead of failing (or being swapped) at the vendor.

| check | rule | proof | status |
|---|---|---|---|
| capability | 400 `capability_unsupported`: text alone on an image model without `text_to_image` or a video model without `text_to_video`; an `init` image on an image model with neither `image_to_image` nor `inpainting`; a frame (`first_frame`, `last_frame`, `init`, `keyframe`) on a video model without `image_to_video`; no reference on a 3D model without `text_to_3d` (a required `init` role still answers `ref_role_required` first) | validator `a_capability_the_model_lacks_is_refused_by_name` | ✅ |
| `n` | litegen returns one image per request, and a vendor asked for more bills them all, so `n > 1` is 400 `param_out_of_range` (param `n`) | validator `more_than_one_image_per_request_is_refused` | ✅ |
| a last frame alone | 400 `ref_role_required` (param `reference_images`) for a `last_frame` with no `first_frame` or `init` image on a video row whose `last_frame` role says `needs_first_frame` (every row of BytePlus, fal FLUX 3, Ideogram, Kling, PixVerse, Runway's resold models, TokenHub and Vidu), at validation, before the quote, reservation and prompt check; Luma, Bedrock's Ray 2 and MiniMax take a last frame alone. The adapters still refuse it as a backstop | loader `needs_first_frame_is_a_video_last_frame_rule`; HTTP `a_last_frame_without_a_first_is_refused_before_it_is_quoted_or_reserved` (every catalog row with a `last_frame` role) | ✅ |
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
| refused before any call (`InvalidRequest`, `ModelNotSupported`, `PricingUnavailable`) | 400 `validation_error` / `request_unsupported`, `model`, `retryable: false`; does not spend an `Idempotency-Key`; never counts toward the circuit breaker (E9) | message | HTTP `an_adapter_refusal_does_not_spend_its_key`; router `a_refusal_before_any_call_is_not_a_vendor_attempt`, `an_adapter_refusal_does_not_count_against_the_breaker`; existing adapter tests | ✅ |
| no usable credential at call time (`NotConfigured`) | 400 `provider_not_configured`, naming the provider, with the adapter's advice (E9) | message | router `a_missing_credential_is_provider_not_configured`; handler `an_adapters_credential_advice_reaches_the_caller` | ✅ |
| every breaker on a route open, or every provider missing | 502, retryable, naming each skip; does not spend an `Idempotency-Key` (E9b) | message | router `a_route_whose_every_breaker_is_open_answers_a_retryable_502_and_spends_no_key` | ✅ |
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
| per megapixel | `fal/flux-pro` 0.04, `fal/flux-dev` 0.025, `fal/flux-schnell` 0.003, `fal/flux-2` 0.012, `-turbo` 0.008, `-flash` 0.005, `-klein-4b` 0.005 (edits 0.01), `-klein-9b` 0.006 (edits 0.011), `-klein-4b-base` 0.009, `-klein-9b-base` 0.011, `fal/sd35-medium` 0.02; the FLUX.2 edits add each input image at its 1 MP ("Input images will be resized to 1MP") | `per_megapixel_usd`, `add_usd` by `references` | fal's "per megapixels" (llms.txt); how fal counts or rounds megapixels is undocumented **(unverified)** | 🟡 |
| per second | `runway/gen4.5` 0.12, `runway/gen4-turbo` 0.05; `kling/video-kling-v2-5-turbo`, `-v2-6`, `-v3` by `resolution` / `audio` (2026-11-03; `extra.mode` / `extra.sound` before); `minimax/MiniMax-H3`, `-H3-Max` by `resolution`; `vidu/viduq3-turbo` 0.06 (540p 0.035, 1080p 0.065), `viduq3-pro` 0.10 (540p 0.045, 1080p 0.12), `viduq2-pro-fast` $0.03 + $0.01/s (1080p $0.06 + $0.02/s) (2026-11-09: Vidu's credits a second, `varying_rows_price_the_variant_asked_for`); `fal/ltx-2.3` by `resolution`; `pixverse/v6`, `pixverse/c1` by `resolution` and `extra.generate_audio_switch` | `per_second_usd` (+ `add_usd`) | each row's quoted vendor rate | 🟡 |
| fixed tiers | `pixverse/v5` (5 s / 8 s × resolution), `pixverse/v5-multi-transition` (1–10, 15, 20, 25, 30 s × resolution), `ideogram/ideogram-4-5-precise-edit` (quality), `hunyuan/hunyuan-video-i2v` (480p 2 credits, 720p and 1080p 5) | `usd` by value | rows' quoted tables; conformance `a_480p_hunyuan_image_to_video_job_is_billed_at_two_credits` | 🟡 |
| multipliers and add-ons | `fal/recraft-v3` vector style ×2, `fal/hyper3d-rodin` HighPack ×3, `fal/hunyuan3d-v2` textured ×3, `fal/hunyuan3d-v3.1-pro` +$0.15 each for PBR, views, a face count, `fal/tripo-h3.1-multiview` $0.20 untextured | `multiply`, `add_usd`, `usd` | fal llms.txt quotes | 🟡 |
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
| `Idempotency-Key` | 1–255 visible ASCII characters, scoped to the caller's org, app and key. Same key + same request (method, path, canonical body) within 24 h: the stored response, byte for byte, with `Idempotent-Replayed: true`, and nothing runs or bills. Same key + another request: 409 `idempotency_key_reused`. Same request still running: 409 `idempotency_request_in_progress` with `Retry-After`. A request refused before any vendor call (validation, auth, quota, unconfigured provider) does not spend the key. A record left `in_progress` for 15 min is abandoned. Every billed POST, with a key or without one, runs to its end in a task of its own: a client that hangs up mid-call does not cancel the vendor call, the request is charged only if it succeeds, its reservations are settled, and it shows in the request log, `GET /v1/generations` and the key's webhook as if the client had stayed. | HTTP: `tests/idempotency_api.rs` (all three families replay with one vendor job and one log row; conflict; unspent key on a validation error; malformed key; a timed-out call replayed as its 504; `a_client_that_hangs_up_without_a_key_leaves_nothing_reserved`). Drop guards: `an_unsettled_guard_releases_its_reservations_when_dropped`, `an_unsettled_quota_hold_is_given_back_when_dropped`; span: `a_billed_request_keeps_its_span_in_its_own_task`. storage: `a_finished_request_replays_and_a_new_body_conflicts`, `a_released_key_can_run_again`, `a_record_older_than_a_day_is_forgotten_and_a_stale_run_abandoned`, `racing_first_requests_start_exactly_once`, Postgres `idempotency_records_against_postgres`. hashing: `a_json_body_hashes_the_same_whatever_its_layout`, `a_multipart_body_hashes_the_same_whatever_its_boundary`, `keys_belong_to_their_owner`. wire: `every_billed_post_documents_its_idempotency_key`. SDKs: `idempotencyKey` (TS `Idempotency-Key` tests), `idempotency_key` (Python, sync and async). | ✅ |

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

## Response headers (L19)

Every one is exposed to cross-origin scripts (`api::cors`), so a browser SDK
reads what a server-side one does. The SDKs hand every response to
`onResponse` / `on_response`, read these with `readResponseMeta` /
`response_meta`, and put an error's headers on `LiteGenAPIError.headers`.

| header | when | SDK field | proof | status |
|---|---|---|---|---|
| `X-Litegen-Dropped-Params` | `strict: false` dropped params | `droppedParams` / `dropped_params` | cors `a_browser_can_read_every_header_the_sdks_report`; TS and Python L19 contract tests | ✅ |
| `X-RateLimit-Limit`, `-Remaining`, `-Reset` (Unix seconds) | a key with an rpm limit | `rateLimit` / `rate_limit` | same | ✅ |
| `Idempotent-Replayed: true` | a stored answer replayed for an `Idempotency-Key` | `idempotentReplayed` / `idempotent_replayed` | same | ✅ |
| `Retry-After` | a 429 or 503 | `retryAfterMs` / `retry_after` | same | ✅ |
| `X-Litegen-Conversion-Losses`, `Content-Disposition` | a mesh conversion | `models3d.convert` | cors test; existing convert tests | ✅ |

A browser may send every header the SDKs set — `Idempotency-Key`,
`X-Litegen-Org-Id`, `X-Litegen-App-Id`, `X-Litegen-End-User`,
`X-Litegen-Tags`, `X-CSRF-Token`, `X-Litegen-Strict` — with every method they
use, PUT included (`a_browser_may_send_every_header_the_sdks_set_with_every_method_they_use`).

The SDKs' default timeouts cover the server's worst case: a request waits 6.5
minutes (three vendor attempts of up to 120 s), a poll 2 h 5 min (past the
poller's 2 h reap age).

## Request metadata (L17)

`metadata` is the caller's own labels for a call, on every generation
(image, video, 3D) and on segmentation. It never reaches a vendor.

| what | rule | proof | status |
|---|---|---|---|
| shape | a JSON object of string values; keys of 1–64 letters, digits, `_`, `.` and `-` (`param_out_of_range` otherwise); at most 1 KB as JSON (`param_too_long`); refused before billing | HTTP `a_jobs_metadata_is_kept_echoed_and_splits_logs_and_spend` | ✅ mock |
| kept | on the generation row as `metadata.request.metadata`, and on the request log as its `metadata` | same; Postgres `metadata_filters_and_refund_claims_against_postgres` | ✅ |
| echoed | `metadata` on the image, video, 3D and segmentation answers, at submit and on every read; webhooks carry the row, so `metadata.request.metadata` | same; poller `the_terminal_webhook_carries_the_callers_metadata`; handler `a_cancel_refunds_the_job_and_sends_its_webhook` | ✅ |
| filters | `metadata.<key>=<value>` on `GET /v1/logs` and `GET /v1/stats`, repeatable, every pair held; a key metadata cannot hold is a 400 | HTTP (sqlite, with an app) and Postgres (with and without one); SDKs `logs.list({ metadata })` / `stats.get({ metadata })`, Python `metadata=` (sync and async) | ✅ |

## Cancelling and refunds (L18)

`PATCH /v1/generations/{id}` with `{"status": "cancelled"}` cancels the row,
asks the vendor to stop (waiting up to 5 s for its answer), and reports
`vendor_cancelled`: true only when the vendor took it. A job that is
cancelled or fails — at any observer: the poller, a reap, a client's poll, a
cancel — gives back what it was billed at submit, once: the key's quota, the
limits of every principal it was charged against (recorded on the row at
submit as `billing.limits`), and the row's and log's `cost_usd`, so `usage`
and `/v1/stats` read 0. A cancel also sends the terminal webhook.

| vendor | cancel | `vendor_cancelled: true` when | proof | status |
|---|---|---|---|---|
| fal (video and 3D) | `PUT {response_url}/cancel` | 200 `OK` or 202 `CANCELLATION_REQUESTED`; 400 `ALREADY_COMPLETED` and 404 are false | `cancel_puts_to_the_jobs_queue_url_and_maps_fals_four_answers` (video), `cancel_maps_fals_three_answers` (3D) | ✅ wiremock |
| Replicate (video) | `POST /v1/predictions/{id}/cancel` | 2xx, unless the prediction it returns had already `succeeded` or `failed`; the reference documents no response **(unverified)** | `cancel_posts_to_the_predictions_cancel_endpoint` | 🟡 wiremock |
| Runway (video) | `GET /v1/tasks/{id}`, then `DELETE` only a task still `PENDING`, `THROTTLED` or `RUNNING` (the DELETE would delete a finished one, refunding nothing) | 204 | `cancel_deletes_a_task_that_has_not_finished`, `cancel_leaves_a_finished_task_alone_and_reports_one_that_is_gone` | ✅ wiremock |
| mock video | always taken | always | HTTP `a_cancelled_job_is_stopped_at_the_vendor_and_refunded` | ✅ |
| mock 3D and every other vendor | none wired | never | HTTP `a_failed_job_is_refunded_and_a_vendor_without_a_cancel_says_so` | — |

| refund | proof | status |
|---|---|---|
| on cancel: quota, row and log cost, stats | HTTP `a_cancelled_job_is_stopped_at_the_vendor_and_refunded`; handler `a_cancel_refunds_the_job_and_sends_its_webhook` | ✅ |
| on failure seen by a client's poll | HTTP `a_failed_job_is_refunded_and_a_vendor_without_a_cancel_says_so` (`mock/priced-fail-3d`, $0.25) | ✅ |
| on a reap, once however many observers settle it | poller `a_reaped_job_is_refunded_once` | ✅ |
| limits of every principal charged | management `a_cancelled_job_refunds_every_principal_it_was_charged_against` | ✅ |
| the claim, in Postgres | `metadata_filters_and_refund_claims_against_postgres` | ✅ |

## Dashboard Playground controls (Task 43)

Every request field a catalog row can take has a control in the Playground
(`dashboard/src/playground/`): a structured param edits a draft that
`wireValue` turns into the wire form, and a half-filled one is not sent; a
complete one goes as typed. Single mode runs image, segmentation
(`/v1/images/segmentations`), video and 3D models; Compare mode compares image
and 3D generation. Before Generate, the exact body goes to the family's quote
route (`/v1/{images,videos,models3d}/cost`, which validates as the generation
does and calls no vendor): a refusal holds Generate back in the server's words
(E11b; vitest `server-check.test.ts`, e2e `Single Mode: an operation with no
mesh to work on is held back, in the server's words`). The page keeps no copy
of the server's rules.

| field | control | proof | status |
|---|---|---|---|
| `region` | x / y / width / height in pixels of the input image | vitest `sends a region only once all four sides are whole pixels`; e2e `a region edit sends the box, the mask growth and the kept pixels` (`mock/region-image`) | ✅ mock |
| `mask_grow_px`, `preserve_unmasked`, `feather_px` | offered as `GET /v1/models/{id}` serves them in `litegen_params`, with their bounds (a model that takes a mask or a region; no catalog param declares them); the page derives none | vitest `takes each field and its bounds from litegen_params`, `derives nothing from the model's shape`; same e2e | ✅ mock |
| `expand` | pixels per side, up to `max_px` | vitest `sends the sides of an expand that grow…`; e2e `an expand sends the sides that grow` (`mock/expand-image`) | ✅ mock |
| `layout` | rows of kind, word or prompt, and a box as fractions, sent as typed once its four numbers are set; a box off the canvas is held back in the server's words | vitest `sends layout boxes as typed fractions…`; e2e `layout boxes go as fractions…` (`mock/layout-image`) | ✅ mock |
| `palette` | colour swatches, up to `max_colors`, sent as typed | vitest `sends a palette as typed…`; e2e `a palette constrains the pixel art…` (`mock/pixel-art-image`) | ✅ mock |
| `background`, `output_format`, `views`, `skeleton`, `joint_naming`, `camera`, `replace`, `direction` | the generic select or set | e2e `background and output format go as chosen`, `multiview views go as the set ticked`, `a mesh of mine is rigged…`, `a clip runs with its camera, audio and loop…` | ✅ mock |
| `duration_seconds` with listed `values` | only the listed lengths | e2e `a clip runs with its camera, audio and loop, at a listed length` (`mock/sprite-video`) | ✅ mock |
| `audio`, `loop` | checkboxes | same | ✅ mock |
| keyframe `reference_images[].time_s` | a time beside each keyframe image; blank lets the model spread them | vitest `sends each keyframe its time, and no other role one`; e2e `keyframes go with the times they are pinned at` (`mock/keyframe-video`) | ✅ mock |
| 3D `view-*` `reference_images` | an upload or URL row per view the model declares; Generate is held while the server's check refuses the body (`check_multiview_refs`: an input image beside views, views without the front, a gap in a model's `view_order`), in its words | e2e `Single Mode: multiview-to-3D from uploaded views, each under its own side` (`mock/all-params-3d`: "needs a view-front image", "not both") | ✅ mock |
| `source_model`, `source_video` | a picker of the caller's completed generations of that kind (by id), a pasted id, or a URL with its format | vitest `sends a source by generation id or URL…`; e2e `a mesh of mine is rigged, then animated…`, `a clip of mine is picked by generation id to extend, then to retake…` | ✅ mock |
| `time_range` | from / to in seconds, sent as typed | vitest `sends a time range as typed once both ends are numbers…`; same e2e (`mock/retake-video`) | ✅ mock |
| `animations` | rows of preset or motion prompt, name, loop, in place; a typed name goes as typed, a blank one not at all: the server names the clip (`mock:walk` → `walk`) and judges a typed name | vitest `sends a clip name as typed, and none for a blank one…`; same e2e (`mock/animate-3d`: sent unnamed, delivered `walk`) | ✅ mock |
| segmentation `prompts` | rows of point, box or text, of the kinds the model takes; which go together is the server's to say (a text prompt mixed with a point is refused at submit, in its words) | vitest `builds a segmentation request…`; e2e `an image is segmented by a point and a box…` (`mock/segment-image`) | ✅ mock |
| provider passthrough `extra` | one row per `extra_allowlist` key; a key typed in `extra_params` gets a control of its type (listed values, yes/no, a number) and goes as that type (fal FLUX's `safety_tolerance` as the string `"2"`), an untyped one as JSON where its text parses | vitest `typed passthrough fields (extra_params)` | ✅ mock |
| results | an image, a clip (a GIF as an image), masks with scores and boxes, or the 3D inspector with the scene manifest, clips and the vendor's `original` file | vitest `scene-manifest.test.ts`; e2e above (`playground-video`, `playground-masks`, `playground-mesh-scene`, `playground-mesh-clips`) | ✅ mock |
| model detail | `task`, every served capability flag, and the catalog facts (keyframes, durations, region input, mask convention, kept pixels, texture resolutions, layout and prompt kinds, lifecycle); segmentation's own endpoint and an operation's source in the copied curl | vitest `the catalog fields the drawer shows`, `segmentation and operation models`; e2e `states an operation's task, and a keyframe model's keyframes and lengths` | ✅ mock |

## Image / Video

Row inventory generated 2026-08-27, recounted 2026-10-30 (the same query, read with the `yaml` package), via:

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
image   aspect_ratio     aspect_ratio  29  bfl/flux-kontext-max
image   guidance_scale   float         13  fal/auraflow
image   negative_prompt  string        25  fal/sd35-medium
image   quality          string         6  ideogram/ideogram-4-5-precise-edit
image   seed             seed          49  bfl/flux-2-klein-9b
image   size             size          35  bfl/flux-2-klein-9b
image   steps            int           12  fal/auraflow
image   strength         float          5  leonardo/diffusion-xl
image   style            string        10  fal/recraft-v3
video   aspect_ratio     aspect_ratio  26  bytedance/doubao-seedance-1-0-pro-250528
video   duration_seconds float        39  bytedance/doubao-seedance-1-0-pro-250528
video   duration_seconds int            4  minimax/MiniMax-H3
video   fps              int            4  fal/ltx-2.3
video   negative_prompt  string         4  kling/video-kling-v2-5-turbo
video   resolution       string        31  bytedance/doubao-seedance-1-0-pro-250528
video   seed             seed          13  bytedance/doubao-seedance-1-0-pro-250528
```

### fal adapters (audit 2026-10-30)

| behaviour | rule | proof | status |
|---|---|---|---|
| owned fields | `extra` adds an endpoint's knobs but never replaces `prompt`, `image_size`, `seed`, `image_url`, `duration`, …; `num_images` and `sync_mode` are never sent (fal bills every image it makes; litegen keeps one) | `extra_cannot_replace_what_litegen_sends_and_num_images_is_never_sent` (image), `extra_cannot_replace_what_litegen_sends` (video) | ✅ wiremock |
| no second billed run | a send that may have reached fal, and any result download after a 2xx, is never retried; a sync 5xx follows `X-Fal-Needs-Retry` | `a_result_that_will_not_download_after_fal_ran_is_never_run_again`, `fals_errors_are_read_whole_whatever_their_shape` | ✅ wiremock |
| errors | every fal error shape (a `detail` array, a `detail` string with `error_type`, a gateway page) is read whole before its status is judged | same; video `fals_verdicts_fail_the_job_and_a_gateway_page_is_asked_again` | ✅ wiremock |
| queue URLs | the URLs fal returned only when they are on the configured base; else rebuilt with the app id (a URL with the endpoint path is a 405) | `a_job_is_polled_and_cancelled_at_the_configured_base_by_its_app_id` | ✅ wiremock |
| verdicts | `COMPLETED` with `error`, or a result answering the model's error, fails the job on every arm; a gateway page is asked again | `fals_verdicts_fail_the_job_and_a_gateway_page_is_asked_again` | ✅ wiremock |
| result downloads | public addresses only, at most 64 MiB, no redirects | `a_result_is_fetched_only_from_a_public_address` | ✅ |
| hosts | images run on fal's sync host (`https://fal.run`, or `options.sync_api_base`); `api_base` is the queue host video and 3D share | `images_run_on_fals_sync_host_beside_its_queue_host` | ✅ |
| health | every fal adapter checks the key against the Platform API (`/v1/models/pricing`) | `health_check_validates_the_key_against_the_platform_api` (image, video, 3D) | ✅ wiremock |

### Google and OpenAI image adapters (audit 2026-10-30)

`extra` is filtered to the model's allowlist in both modes: strict refuses
another key (`extra_key_unsupported`), lax drops it and reports `extra.<key>`
in `X-Litegen-Dropped-Params` (`extra_lax_keeps_only_allowlisted_keys_and_reports_the_rest`).

| behaviour | rule | proof | status |
|---|---|---|---|
| one image asked for | Google sends no `candidateCount`, OpenAI `n: 1` on both paths: litegen returns and bills one | `one_image_is_asked_for_and_extra_cannot_replace_what_litegen_sets` (Google), `one_image_is_asked_for_and_extra_cannot_replace_what_litegen_sends` (OpenAI) | ✅ wiremock |
| owned fields | `extra` never replaces `model`, `prompt`, `n`, `size`, `quality`, `responseModalities`, `imageConfig`, …; only `imageSize` nests in Google's `imageConfig` (its only other member is `aspectRatio`); OpenAI's multipart edit takes no `moderation` | same | ✅ wiremock |
| the final image | Google's last non-`thought` image; with none, `finishReason` / `blockReason` is the error, with Google's answer | `the_last_finished_image_is_kept_and_a_refusal_says_why` | ✅ wiremock |
| reference types | a Google reference's `mimeType` is its bytes' type | `a_reference_is_labelled_with_its_real_type` | ✅ wiremock |
| no second billed call | a call that timed out may have run: only a connection that never opened is retried | `a_timed_out_call_is_never_sent_again` (Google, OpenAI) | ✅ wiremock |
| OpenAI URLs | built from the API root `api_base` (`https://api.openai.com/v1`; a generations URL still names the same root), health check included | `every_url_is_built_from_the_configured_root` | ✅ wiremock |
| result downloads | OpenAI's legacy URL results: public addresses only, at most 64 MiB | `a_result_url_is_fetched_only_from_a_public_address` | ✅ |
| catalog | `gemini-3.1-flash-lite-image` takes no `imageSize` (1K only); every Google and OpenAI row names its docs page; OpenAI rows take `safety_identifier` (`user` is deprecated) | `gemini_flash_lite_image_takes_no_image_size`, `every_google_and_openai_row_names_its_docs_url`, `openai_rows_take_a_safety_identifier` | ✅ |
| drift | a model the vendor's own page or spec says "was shut down" is not live (Veo 3.1 previews, `gpt-image-1`); Google's "(Shut down)" rows are skipped; OpenAI's deprecations page is a `lifecycle` source | `scripts/model-drift/test/shutdown.test.mjs` | ✅ |

### ByteDance, MiniMax and Vidu adapters (audit 2026-10-30)

| behaviour | rule | proof | status |
|---|---|---|---|
| a last frame needs a first | Vidu img2video would use a lone last frame as the start; BytePlus requires two `image_url`s with roles: refused, nothing sent | `a_last_frame_without_a_first_is_refused_before_any_call` (Vidu, ByteDance) | ✅ wiremock |
| frame types | a base64 frame is a `data:` URI of its bytes' own type (all five adapters) | `a_base64_frame_is_sent_with_its_own_type`, `an_image_request_sends_only_what_its_endpoint_takes`, `frames_go_with_their_own_type_and_only_the_fields_their_mode_takes`, `the_answer_comes_inline_and_a_reference_goes_with_its_own_type` | ✅ wiremock |
| only what the endpoint takes | Vidu sends `aspect_ratio` only to text2video and reference2video; MiniMax S2V-01 gets no `duration`/`resolution`; Hailuo first-and-last-frame requests get no `fast_pretreatment`; MiniMax image gets no `n` | same; `subject_to_video_sends_no_duration`, `one_image_is_asked_for` | ✅ wiremock |
| Hailuo lengths | 6 or 10 s, and only 6 s at 1080P: any other length is refused | `a_hailuo_length_it_cannot_make_is_refused` | ✅ |
| owned fields | `extra` cannot replace `model`, `prompt`, `content`, `images`, `response_format`, … (Seedream's `response_format` is no longer allowlisted) | the tests above; conformance `seedream_rows_do_not_allowlist_response_format` | ✅ |
| honest endings | a success with no URL or file is `failed`; a finished job reads 100; BytePlus `cancelled`/`expired` and MiniMax's failed file lookups say so (a lookup failure is retried) | `a_finished_task_reads_100_and_says_why_it_ended`, `a_success_without_a_url_fails_and_a_finished_job_reads_100`, `a_success_without_a_file_fails_and_a_failed_lookup_is_retried` | ✅ wiremock |
| no second billed submit | a submit that timed out is never sent again | `a_timed_out_submit_is_never_sent_again` (all five) | ✅ wiremock |
| catalog | no Vidu row allowlists `off_peak` (48 h, against litegen's 2 h reap) or `style`; Q2's bodies have no `bgm`/`movement_amplitude`; Q1 takes 3:4 and 4:3; H3-Max takes ≤ 9 reference images; every row names its docs page | `vidu_rows_allowlist_only_fields_their_bodies_take`, `vidu_3_4_and_4_3_are_only_advertised_for_q1_and_q2_models`, `minimax_h3_max_has_no_2k_no_4s_and_takes_references`, `every_bytedance_minimax_and_vidu_row_names_its_docs_url` | ✅ |
| drift | BytePlus's restructured model list and pricing are read again (the check failed with exit 2); glued "new" badges and blockquote notes are not content | `scripts/model-drift/test/bytedance.test.mjs` | ✅ |

### Kling, PixVerse, Hunyuan and Luma adapters (audit 2026-10-30)

| behaviour | rule | proof | status |
|---|---|---|---|
| what a model cannot make | refused before any call or upload: a lone last frame (Kling, PixVerse); a PixVerse last frame outside c1, v6, v5.6 and v5 (v3.5 and v4.5 carry no `last_frame` role); a Kling 2.5 Turbo / 2.6 last frame below 1080p; a duration the model does not take (Kling 5 or 10 s and whole seconds; PixVerse v3.5 to v5 5 or 8 s; Luma 5 or 9 s); a Luma loop with a last frame | `what_a_model_cannot_make_is_refused_before_any_call` (Kling), `what_a_model_cannot_make_is_refused_before_any_upload` (PixVerse), `what_luma_cannot_make_is_refused_before_any_call` | ✅ wiremock |
| only what the endpoint takes | Kling and PixVerse send `aspect_ratio` to text-to-video only; PixVerse's transition body has no `generate_multi_clip_switch`; Luma never sends `camera_motion`; Kling image sends `n: 1`, its subject reference as `image_reference: "subject"`, and refuses a negative prompt with a reference | `only_text_to_video_gets_an_aspect_ratio`, `the_transition_body_has_only_what_transition_takes`, `camera_motion_is_never_sent_as_a_field`, `one_image_is_asked_for_and_a_subject_is_sent_as_one`, `a_negative_prompt_with_a_reference_is_refused` | ✅ wiremock |
| owned fields | `extra` never replaces the fields each adapter writes (model, prompt, frames, duration, quality, …) | the tests above | ✅ wiremock |
| Hunyuan seed | a seed is 1 to 2147483647 (`Seed`: unset is random, a positive seed is fixed); it has no effect while `Revise` is on, the default, and never with a reference image | `only_a_positive_seed_is_sent`; conformance `hunyuan_image_seeds_are_positive` | ✅ wiremock |
| retries | a submit the vendor turned away (Kling 1302–1304 and 5000–5002, PixVerse 500010, 500044 and 500069, Luma 429, Tencent `InternalError`, `RequestLimitExceeded` and `ServiceUnavailable`) may be sent again; one that may have run is not | `a_rate_limited_submit_may_be_sent_again`, `a_turned_away_submit_may_be_sent_again` (Kling); `a_submit_turned_away_at_the_limit_may_be_sent_again` (PixVerse); `an_error_says_luma_s_detail_and_a_rate_limit_may_be_sent_again`; `a_passing_tencent_error_is_asked_again_and_a_verdict_is_not` | ✅ wiremock |
| honest polls | a poll error is an error that says whether to ask again (Kling, PixVerse `ErrCode`); PixVerse status 7 and 8 say which; a success without a URL fails; a failed Hunyuan image job gives `JobErrorMsg` | `a_poll_error_is_an_error_that_says_whether_to_ask_again`, `a_poll_error_is_an_error_and_a_failure_says_which`, `a_failed_job_says_why` | ✅ wiremock |
| hosts | Kling's default host is `https://api-singapore.klingai.com` | `the_default_host_is_kling_singapore` | ✅ |
| catalog | `hunyuan/hunyuan-image-3` is retired (its aiart actions were deleted 2026-09-22); a 480p Hunyuan I2V job bills its 2 credits; PixVerse prompts stop at 2048 (img/), v3.5 to v5 list 5 and 8 s, and no row allowlists `negative_prompt`, `motion_mode`, `style` or `template_id`; Hunyuan video is 5 s only; every Luma video row takes `loop` and a 3–5000-character prompt; every row of the four vendors names its docs page | `no_retired_model_ids_are_advertised`, `pixverse_v4_5_and_v3_5_take_no_last_frame`, `pixverse_prompts_stop_at_the_image_endpoint_s_2048`, `pixverse_v5_and_older_take_five_or_eight_seconds`, `pixverse_allowlists_name_only_fields_their_endpoints_bill_flat`, `hunyuan_video_rows_are_five_seconds_only`, `every_luma_video_row_takes_loop`, `luma_video_prompts_are_three_to_five_thousand_characters`, `every_kling_pixverse_hunyuan_and_luma_row_names_its_docs_url`, `a_480p_hunyuan_image_to_video_job_is_billed_at_two_credits` | ✅ |

### Runway, Leonardo and Replicate adapters (audit 2026-10-30)

| behaviour | rule | proof | status |
|---|---|---|---|
| owned fields | `extra` adds a vendor's knobs but never replaces what litegen sends: Runway's `model`, `promptText`, `ratio`, `promptImage`, `duration`, `seed`, `referenceImages`; Leonardo's `modelId` (no longer allowlisted: it would run any Leonardo model at Diffusion XL's price), `prompt`, `num_images`, `width`, `height`, `model`, `resolution`, `duration`; Replicate's `input` fields | `extra_cannot_replace_what_litegen_sends` (Runway image and video, Leonardo video, Replicate video), `one_plain_image_is_asked_for_and_extra_cannot_replace_what_litegen_sends` (Leonardo image), `the_input_says_what_each_model_takes_and_extra_cannot_replace_it` (Replicate image) | ✅ wiremock |
| Leonardo images | one image (`num_images` defaults to 4) and Alchemy off unless `extra.alchemy` asks (it defaults on and makes the output 1.75× `size`) | `one_plain_image_is_asked_for_and_extra_cannot_replace_what_litegen_sends` | ✅ wiremock |
| references | a Runway reference is a URL or a data URI of a JPEG, PNG or WebP of at most 5,242,880 characters (no GIF); a Leonardo upload names the image's own `extension`; a Replicate data URI has its bytes' type; anything a vendor cannot take is refused before any call | `a_reference_goes_with_its_own_type_and_one_runway_cannot_take_is_refused`, `a_frame_goes_with_its_own_type_and_one_runway_cannot_take_is_refused`, `a_reference_goes_up_as_its_own_type_and_a_gif_is_refused`, `the_input_says_what_each_model_takes_and_extra_cannot_replace_it` | ✅ wiremock |
| Replicate strength | img2img strength is `prompt_strength` | `the_input_says_what_each_model_takes_and_extra_cannot_replace_it` | ✅ wiremock |
| no second billed submit | a submit that timed out is never sent again; a 429 or 5xx was turned away and may be; Leonardo's errors carry its status | `a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be` (Runway), `a_turned_away_create_says_so_and_a_timed_out_one_is_never_sent_again` (Leonardo), `a_timed_out_create_is_never_sent_again_and_a_throttled_one_may_be` (Replicate) | ✅ wiremock |
| honest polls | a poll error is that error, not a job still running (a 5xx or 429 inside an image call is asked again); Runway's `progress` is a fraction; Replicate's `aborted`, or a status outside its enum, is an ending; a success without an output (Runway, Replicate) or without `motionMP4URL` (Leonardo) fails | `a_poll_error_is_an_error` (Runway, Leonardo images), `a_poll_says_what_runway_said`, `a_poll_says_what_leonardo_said`, `an_aborted_prediction_is_an_ending`, `an_aborted_unknown_or_empty_prediction_fails` | ✅ wiremock |
| Replicate polls | at the configured `api_base` only, never at the `urls.get` a response names (the key would go to that host) | `a_prediction_is_polled_only_at_the_configured_base` | ✅ wiremock |
| model ids | `replicate/video` has its own arm and an unknown id is refused, not run as AnimateDiff | `every_catalog_row_resolves_and_an_unknown_id_does_not` | ✅ |
| result downloads | images are fetched without the key, from public addresses only, at most 64 MiB | `a_result_is_fetched_only_from_a_public_address` (Runway, Leonardo, Replicate) | ✅ |
| catalog | gen4.5 takes no `outputFormat` / `proresProfile` (other containers at a surcharge); gen4-turbo's prompt is optional and an empty one is not sent; Diffusion XL allowlists no `modelId` or `contrast`; Motion 2.0 says it is sent at 720p; every row names its docs page | `runway_gen4_5_returns_only_the_mp4_it_prices`, `runway_gen4_turbo_takes_an_optional_prompt`, `gen4_turbo_without_a_prompt_sends_no_prompt_text`, `leonardo_diffusion_xl_cannot_name_another_model_or_a_phoenix_knob`, `leonardo_motion2_says_it_is_sent_at_720p`, `every_runway_leonardo_and_replicate_row_names_its_docs_url` | ✅ |

### Runway resold image models (2026-11-10)

Twenty third-party models on `POST /v1/text_to_image` (`image/runway.rs` `RESOLD`), each sent its own branch of the request union.

| behaviour | rule | proof | status |
|---|---|---|---|
| `aspect_ratio` × `resolution` → `ratio` | each cell is one size of the branch's `ratio` enum (generated table per model: the named aspect within 4.5 %, the tier by pixels, GPT Image by Runway's own 1K/2K/4K lists); the row's defaults when none is asked; a row with no default aspect (Ideogram 4.0 / 4.5, Bria FIBO Edit) sends none unless asked; Ideogram Vector sends none | `every_aspect_and_resolution_a_resold_row_offers_reaches_runway_as_its_size` (456 cells), `a_resold_model_sends_its_own_branchs_fields` | ✅ wiremock |
| `quality`, `background`, `output_format`, `seed` | `quality`, `background`, `outputFormat`, `seed`, only where the row declares them | `a_resold_model_sends_its_own_branchs_fields` | ✅ wiremock |
| references | `referenceImages[]` as `{uri, tag}` on GPT Image and Gemini, `{uri}` elsewhere (`additionalProperties: false`); never `outputCount` | `a_resold_model_sends_its_own_branchs_fields` | ✅ wiremock |
| mask | Ideogram 4.5's mask → `maskUri` (black edits), not a reference image | `an_ideogram_mask_goes_as_mask_uri` | ✅ wiremock |
| impossible requests | Bria FIBO Edit sizes only 2+ images; Ideogram 4.5 `very_low` needs a reference and no `preciseEdit`; Grok `edit` / Ideogram `preciseEdit` need a reference: refused before any call | `a_request_runway_documents_as_impossible_is_refused_before_any_call` | ✅ wiremock |
| result type | read from the bytes: PNG, JPEG, WebP, or SVG (Ideogram Vector) | `the_result_is_labelled_by_its_bytes` | ✅ wiremock |
| price | per quality × tier (× aspect on GPT Image 2.5 / 2.6), + $0.01 per reference image on GPT 2.5 / 2.6 and Grok; Ideogram 4.5's default quality follows its references | `runway_resold_image_rows_are_carried_with_runways_contract` | ✅ |

### Runway resold video models (2026-11-10)

Twenty-five third-party models on `POST /v1/text_to_video` and `/v1/image_to_video` (`video/runway.rs` `RESOLD`): a start frame sends image-to-video, otherwise text-to-video, each with its own branch's fields.

| behaviour | rule | proof | status |
|---|---|---|---|
| `aspect_ratio` / `resolution` → `ratio` / `resolution` | per mode (`Shape`): a size of the branch's `ratio` enum (generated table), the aspect itself (MiniMax H3, Grok 1.5), `auto_<tier>` (Kling 4.0 and Grok 1.5 Lite image-to-video, Wan's tier alone), a `resolution` field (MiniMax H3 / H3 Max, Grok 1.5, HappyHorse image-to-video), or none where the image sets the shape | `every_aspect_and_resolution_a_resold_row_offers_reaches_runway_in_a_shape_it_takes`, `a_resold_model_sends_its_own_branchs_fields` | ✅ wiremock |
| frames | `first_frame` alone → `promptImage` string; with `last_frame` → `[{uri, position: first}, {uri, position: last}]` in that order; a last frame alone is refused | `a_resold_model_sends_its_own_branchs_fields`, `a_request_a_branch_cannot_take_is_refused_before_any_call` | ✅ wiremock |
| references | `reference` images → `references` [{uri}] on the mode that takes them (text-to-video, or image-to-video beside the frame on Kling O3 and 4.0); refused where the chosen mode has none | same two tests | ✅ wiremock |
| `duration_seconds`, `audio`, `seed`, `negative_prompt` | `duration` (int; Veo 4/6/8, Kling 2.5 5/10), `audio`, `seed`, `negativePrompt`, only where the row declares them; `audio` only when asked | `a_resold_model_sends_its_own_branchs_fields` | ✅ wiremock |
| prompt | where text-to-video requires `promptText` but image-to-video doesn't, the row's prompt is optional and says `required_for_text_to_video: true`: a request with no first frame and no prompt is refused at validation (`prompt_required`, N3), before it is quoted or reserved, and the adapter refuses it too; Kling 3.0/O3 and Seedance 2.5, whose text-to-video takes no `promptText`, leave it off | `a_request_a_branch_cannot_take_is_refused_before_any_call`; `the_catalog_says_which_text_to_video_needs_a_prompt_as_runway_does`; HTTP `a_text_to_video_request_with_no_prompt_is_refused_before_it_is_quoted_or_reserved` | ✅ wiremock |
| owned fields | `extra` can't replace `model`, `promptText`, `promptImage`, `duration`, `ratio`, `seed`, `references`, `resolution`, `audio`, `negativePrompt` | `a_resold_model_sends_its_own_branchs_fields` | ✅ wiremock |
| price | per second by tier and audio; + credits per reference image or frame (MiniMax H3 2, Gemini Omni and Grok 1); Seedance 2.5 `draft` at the 480p rate; Kling 4.0 at list price | `runway_resold_video_rows_are_carried_with_runways_contract` | ✅ |

### Bedrock Luma Ray 2 (2026-11-10)

`POST /async-invoke` (`video/bedrock.rs`), signed for us-west-2; the MP4 read back from the caller's S3 bucket.

| behaviour | rule | proof | status |
|---|---|---|---|
| body | `modelId`, `modelInput` {`prompt`, `duration` "5s" / "9s", `resolution` (asked, else 720p), `aspect_ratio` and `loop` when asked, `keyframes` frame0 / frame1}, `outputDataConfig.s3OutputDataConfig.s3Uri`; signed for `bedrock` in us-west-2 whatever the credential names | `ray_2_is_submitted_signed_for_us_west_2_with_its_own_fields` | ✅ wiremock |
| keyframes | `first_frame` → frame0, `last_frame` → frame1, each {type image, source {type base64, media_type, data}}, JPEG or PNG; anything else, or a loop with a last frame, refused before any call | `keyframes_go_as_base64_with_their_own_type` | ✅ wiremock |
| bucket | `s3_output_uri` (options, else the credential's) must be `s3://<bucket>[/<prefix>]` in plain characters, or the request is refused before any call; an id with no arm is refused | `a_request_without_a_usable_bucket_is_refused_before_any_call`, `every_catalog_row_resolves_and_an_unknown_id_does_not` | ✅ wiremock |
| result | a Completed job's `<s3Uri>/output.mp4`, else the first `.mp4` ListObjectsV2 finds under it, read with SigV4 for `s3` (+ x-amz-content-sha256) and kept as bytes; an unreadable one fails naming s3:GetObject / s3:ListBucket; a 5xx is asked again | `a_finished_job_is_read_back_from_the_bucket`, `a_finished_job_elsewhere_under_its_prefix_is_found_and_an_unreadable_one_fails` | ✅ wiremock |
| polls | InProgress / Completed / Failed (with `failureMessage`); an undocumented state fails; a 404 is final, a 5xx retried | `a_poll_says_what_bedrock_said` | ✅ wiremock |
| price | $1.50/s at 720p, $0.75/s at 540p | `bedrock_luma_ray_2_is_carried_with_bedrocks_contract` | ✅ |

### Ideogram resold video (2026-11-10)

`POST /v2/video/generate/{model}-{text,image,reference}-to-video` (`video/ideogram.rs`), polled at `GET /v2/generations/{generation_id}`.

| behaviour | rule | proof | status |
|---|---|---|---|
| routing | reference images → reference to video (`reference_images`), a start frame (+ last) → image to video (`image`, `end_image`), none → text to video; frames with references, a last frame alone, references on Kling refused before any call | `frames_and_references_go_multipart_to_their_own_endpoints`, `a_request_no_endpoint_takes_is_refused_before_any_call` | ✅ wiremock |
| body | text to video JSON, the others multipart: `prompt`, `duration` (always), `aspect_ratio` "16x9" where the endpoint takes one (asked, else the row's default), `resolution` as Ideogram spells it, `generate_audio` and `negative_prompt` when asked, allowlisted extras (`prompt_expansion_mode` on reference to video only); never `webhook_url`; the Api-Key from the pool | `text_to_video_is_json_with_the_fields_its_endpoint_takes`, `frames_and_references_go_multipart_to_their_own_endpoints` | ✅ wiremock |
| polls | pending / completed (the `video.generation` url) / failed (`failure_reason`); a completed one without a url fails; an undocumented state fails; 401 / 404 final, 5xx retried | `a_poll_says_what_ideogram_said` | ✅ wiremock |
| submit errors | a 422 is final, a 5xx may be resent, an answer without `generation_id` is final | `a_refused_submit_says_why` | ✅ wiremock |
| credentials | Ideogram's one dashboard key configures its video adapter | `ideogram_video_takes_the_same_api_key_pool` | ✅ |
| price | per second by tier (Kling by audio); MiniMax H3 + $0.04 per reference image past five | `ideogram_video_resales_are_carried_with_ideograms_contract` | ✅ |

### BFL, Ideogram, Recraft and Stability adapters (audit 2026-10-30)

| behaviour | rule | proof | status |
|---|---|---|---|
| owned fields | `extra` never replaces or doubles what litegen sends: BFL's `prompt`, `width`, `seed`, `output_format`, `input_image`; Ideogram 3.0's `prompt`, `rendering_speed` (QUALITY on a TURBO row would bill another tier), `num_images`, and only the row's allowlist on both 3.0 bodies; Recraft's `model` (another model at this row's price), `n`, `response_format`, `image_format`; Stability's `model`, `output_format`, `prompt` (one part each) and the v1 fields | `extra_cannot_replace_what_litegen_sends` (BFL), `a_3_0_request_sends_only_allowlisted_extras_and_never_over_what_litegen_sends`, `a_raster_image_is_asked_for_as_png_and_extra_cannot_replace_what_litegen_sends`, `a_v2_request_carries_each_field_once` | ✅ wiremock |
| Ideogram multipart extras | with reference images, an allowlisted list (`style_codes`) goes as repeated parts and an object (`color_palette`) as JSON text, not dropped (the spec does not say how multipart carries them **(unverified)**) | `a_3_0_request_with_references_sends_every_allowlisted_extra` | 🟡 wiremock |
| model ids | every Ideogram row has its own endpoint; an unknown id is refused, not run as 3.0 | `every_catalog_row_has_its_own_endpoint_and_an_unknown_id_is_refused` | ✅ |
| BFL statuses | only `Pending`, `Reasoning` and `Generating` keep polling; every other status but `Ready` (`No Card Generated` among them) is an ending | `only_pending_reasoning_or_generating_is_still_running` | ✅ wiremock |
| BFL key | the `polling_url` is polled only on the configured base's origin or an https `*.bfl.ai` host | `the_key_goes_only_to_bfl` | ✅ wiremock |
| Recraft formats | a raster image is asked for as PNG (`image_format` defaults to webp); vector models take none | `a_raster_image_is_asked_for_as_png_and_extra_cannot_replace_what_litegen_sends` | ✅ wiremock |
| Stability moderation | a `CONTENT_FILTERED` (blurred) result fails with Stability's reason, on v2 (`finish-reason` header) and v1 (`finishReason`) | `a_content_filtered_image_fails_with_stabilitys_reason` | ✅ wiremock |
| Stability masks | a mask without an init image is refused before any call | `a_mask_without_an_init_image_is_refused_before_any_call` | ✅ |
| errors | Recraft's plain-text errors and Stability's `errors[]` keep the vendor's words and body; a 429 or 5xx may be sent again | `an_error_says_what_recraft_said_in_whatever_form`, `an_error_says_what_stability_said` | ✅ wiremock |
| no second billed call | a call that timed out is never sent again (BFL, Recraft, Stability); a 429 on BFL's submit may be | `a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be` (BFL), `a_timed_out_call_is_never_sent_again` (Recraft, Stability) | ✅ wiremock |
| result downloads | BFL's `result.sample` and Recraft's URL results: without the key, public addresses only, at most 64 MiB | `a_result_is_fetched_only_from_a_public_address` (BFL), `a_url_result_is_fetched_only_from_a_public_address` (Recraft) | ✅ |
| catalog | Recraft vector rows take no pixel `size` (w:h only), no row allowlists `substyle`, the V3 rows offer no `icon` (V2 only); SDXL takes `strength`; every row names its docs page (Stability's moved to docs.stability.ai) | `recraft_vector_rows_take_no_pixel_size`, `recraft_rows_allowlist_no_substyle`, `recraft_v3_rows_offer_no_icon_style`, `stability_sdxl_takes_an_image_to_image_strength`, `every_bfl_ideogram_recraft_and_stability_row_names_its_docs_url` | ✅ |
| drift | every provider's check exits 0 or 1: Stability's v1 spec is snapshotted as `openapi-v1` (the orphaned `docs-bundle` renamed, byte-identical), and the stale Ideogram v2 and Stability release-notes snapshots are refreshed | `node scripts/model-drift/check.mjs --provider <each>` | ✅ |

### Image

| param | kind | validator rule | # models | Proof | Status |
|---|---|---|---|---|---|
| `aspect_ratio` | AspectRatio | enum `allowed` | 29 | validator: `aspect_ratio_not_allowed`. provider: `ideogram::tests::generates_ideogram_v3_json` (`body["aspect_ratio"]`), `minimax::tests::generates_image_01`, `luma::tests::generates_photon_image_via_polling`, `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["aspect_ratio"]`). conformance: `runway_image_does_not_advertise_unmappable_ratios`, `stability_aspect_ratios_are_in_the_v2_enum`, `minimax_image_aspect_ratios_match_the_documented_enum`. | ✅ |
| `guidance_scale` | Float | range | 13 | validator: `float_out_of_range`. provider: `fal::tests::generates_flux_dev_image` (`body["guidance_scale"]==3.5`), `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["guidance"]==3.5`) and `replicate::tests::guidance_and_steps_use_each_models_own_input_names` (each model's own input name: FLUX `guidance`, SD3 `cfg`, fallback `guidance_scale`; corrected 2026-09-11, the adapter used to send `guidance_scale` to every model). conformance: n/a (not enum). | ✅ |
| `seed` | Seed | range | 49 | validator: `seed_out_of_range`. provider: `fal::tests::generates_flux_dev_image` (`body["seed"]==42`), `bfl::tests::generates_flux_pro_via_polling_url` (`body["seed"]==7`), `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["seed"]==123`). conformance: n/a. | ✅ |
| `size` | Size | enum of `[w,h]` pairs | 35 | validator: `size_enum_must_match`, `size_enum_passes_when_matches`. provider: `bytedance::tests::generates_seedream` (`body["size"]=="2048x2048"`), `openai::tests::generates_dalle3_image_b64_json` (`body["size"]=="1024x1024"`), `recraft::tests::generates_recraftv3_image` (`body["size"]=="1024x1024"`). conformance: **no test** asserts a model's `size` dimension list against the vendor's accepted sizes — real gap. | 🟡 |
| `quality` | String enum | enum + max_length | 6 | validator: **none** — no `validator_tests.rs` case sends `req.quality`. provider: `openai::tests::generates_dalle3_image_b64_json` (`body["quality"]=="high"`). conformance: `openai_image_quality_values_are_the_gpt_image_enum`. | 🟡 |
| `style` | String enum | enum + max_length | 10 | validator: **none** — no case sends `req.style`. provider: `recraft::tests::generates_recraftv3_image` (`body["style"]=="digital_illustration"`); `openai::tests::generates_dalle3_image_b64_json` proves the *absence* case (`style` omitted for dall-e-3). conformance: `ideogram_style_values_match_the_documented_enum`. | 🟡 |
| `negative_prompt` | String | max_length | 25 | validator: **none**. provider: `stability::tests::generates_sd3_large_via_v2_multipart` (asserts the multipart body contains the negative-prompt text, not a strict field-name+value pair). conformance: n/a. | 🟡 |
| `strength` | Float | range | 5 | validator: **none** — no `strength_out_of_range` case exists (only `guidance_scale` is exercised for the Float kind). provider: strong — `stability::tests::image_to_image_sends_mode_image_to_image`, `strength_is_not_sent_for_text_to_image`, `image_to_image_defaults_strength_when_omitted` all assert presence/absence of the outbound `strength` multipart field; SDXL sends it inverted as `image_strength`, to `/image-to-image` only (`sdxl_strength_is_sent_inverted_and_only_to_image_to_image`); Replicate as `prompt_strength`. conformance: n/a. | 🟡 |
| `steps` | Int | range | 12 | validator: `unsupported_param_strict_rejects` / `unsupported_param_lax_drops` prove the shared drop mechanism using `steps` as the literal example — this is genuine, param-named coverage, unlike the other rows above. No test exercises the *range* branch (`steps` out of `[min,max]`) specifically. provider: `fal::tests::generates_flux_dev_image` (`body["num_inference_steps"]==28` — **note the outbound rename `steps` → `num_inference_steps`**), `replicate::tests::generates_flux_dev_via_polling` (`body["input"]["num_inference_steps"]==28`) and `replicate::tests::guidance_and_steps_use_each_models_own_input_names` (each model's own input name: flux-dev `num_inference_steps`, flux-pro and SD3 `steps`, fallback `num_inference_steps`). Not universal: `stability.rs` sets `body["steps"]` (line 466) and the multipart `steps` field (line 375) with zero assertion. conformance: n/a. | 🟡 |

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
| `bytedance/seedream-5-0-lite-260128` (2026-11-03; not 4.0 or 4.5, which the vendor lists as unsupported) | png, jpeg | none (BytePlus's jpeg) | — | `output_format` | `the_output_format_goes_as_its_own_field_and_labels_the_image`; conformance `bytedance_only_seedream_5_0_lite_takes_an_output_format` | 🟡 wiremock |
| `luma/photon-1`, `photon-flash-1` (2026-11-03: Luma's ImageFormat is `jpg \| png`) | jpeg, png | none (Luma's jpg) | — | `format`, jpeg sent as `jpg` | `the_output_format_goes_as_lumas_format`; HTTP `the_listing_says_what_each_model_can_return` | 🟡 wiremock |
| `fal/sdxl` (2026-11-03) | png, jpeg | none (fal's jpeg) | — | `format` (fast-sdxl has no `output_format`) | `fast_sdxl_takes_its_container_as_format` | 🟡 wiremock |
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
| `region` | a box-native vendor gets its own box; a mask-only one the box drawn as the mask; with a mask, where they overlap; every model that takes a mask takes a region (`every_model_that_takes_a_mask_takes_a_region`) | drawn as the mask (`region: {input: mask}` on each mask row since the Task 44 fix pass) | drawn as the mask | drawn as the mask (since Task 44) | `context_window` = `y,x,y+h,x+w`; a region under 256 px a side gets a 256 px window (1:6–6:1, kept on the image) and its exact area as the mask | inpainting: drawn; region: box, 256 px minimum | core `a_small_region_gets_the_vendors_minimum_window_and_its_exact_area_as_the_mask`, `a_window_is_kept_on_the_image_and_within_the_vendors_aspect_ratio`, `a_region_and_a_mask_edit_only_where_they_overlap`; HTTP `a_region_reaches_ideogram_as_its_exact_context_window`, `a_small_region_reaches_ideogram_as_a_256_window_and_its_exact_area_in_black`, `a_region_on_a_mask_model_arrives_drawn_as_its_mask_and_overlaps_a_mask`, `a_small_region_on_a_box_model_gets_its_minimum_window_and_its_exact_area` | 🟡 wiremock / ✅ mock |
| `mask_grow_px` | the vendor's own growth where it has one, sent explicitly (even 0); else litegen dilates (square, both axes); ≤ 100 | litegen dilates | `grow_mask` (default 5 overridden) | litegen dilates | litegen dilates | litegen dilates | core `litegen_grows_the_edit_area_where_the_vendor_cannot`, `a_vendor_that_grows_masks_gets_the_exact_mask_and_an_explicit_growth`; HTTP `stability_grows_its_own_mask_when_asked`, `mask_grow_px_grows_the_edit_area` | 🟡 wiremock / ✅ mock |
| refusals, before billing | `init_required` (400, validator), `mask_size_mismatch`, `mask_ambiguous` (any alpha < 255), `region_out_of_bounds` (400), `region_too_small` (422: no window fits, or a box vendor takes no mask), `image_unreadable` (400) | ✓ | ✓ | ✓ | ✓ | ✓ | HTTP `a_bad_edit_is_refused_before_anything_is_billed` (no row, no log); core tests for each code | ✅ |
| untouched pixels | `preserves_unmasked`: the vendor's own words | — (`unspecified`… not declared) | `unspecified` | — | `exact` ("copied exactly") | `exact`: every other pixel identical | HTTP `the_inpainting_mock_fills_exactly_the_masked_area` (every pixel compared) | ✅ mock |
| listing | `supports_inpainting` (takes a mask), `supports_region_edit`, `region_input`, `preserves_unmasked`, `mask_convention` | | | | | | HTTP `the_listing_says_how_each_model_edits_part_of_an_image`; conformance `the_inpainting_flag_means_the_model_takes_a_mask` | ✅ |

Not verified live: no OpenAI, Stability or Ideogram key is available here.

### Native pixel art (L16)

A row that draws native pixel art says `capabilities.native_pixel_art`
(listed as `native_pixel_art`, with `pixel_size: {min, max}` from its size
range): one image pixel per art pixel, never upscaled. **`palette:
["#rrggbb", …]`** (the row's `palette` spec: `forced` and `max_colors`,
listed as `supports_palette` and `supports_forced_palette`) is every pixel's
colour on a forced row and a hint elsewhere. **`directions`** (4 or 8, the
row's `values`, listed as `directions`) returns one image per facing, each
labelled `direction` — `south` (towards the viewer), then clockwise:
`south-east`, `east`, `north-east`, `north`, `north-west`, `west`,
`south-west` (4: `south`, `east`, `north`, `west`) — from one call, billed
once. Refused before billing: a colour not `#rrggbb` or an empty palette
(`param_out_of_range`), more colours than the row keeps (`param_too_many`), a
directions count the row lacks (`param_enum_mismatch`), either field on a row
without it (`param_unsupported`).

| row | vendor call | price | proof | status |
|---|---|---|---|---|
| `mock/pixel-art-image` | native size (16–128), a checker in the palette's colours only; one labelled image per facing | $0.01 per call | HTTP `a_forced_palette_comes_back_at_native_size_in_only_its_colours` (16 colours), `eight_directions_come_back_as_eight_labelled_images` (billed once), `a_palette_or_direction_count_the_model_cannot_take_is_refused_before_billing`, `the_listing_says_which_models_draw_native_pixel_art` | ✅ mock |
| `pixellab/pixflux` | `POST https://api.pixellab.ai/v2/create-image-pixflux` (Bearer) `{description, image_size {width, height} 16–400, color_image: {type: base64, base64, format: png}` — the palette as a strip one pixel per colour — `, no_background` for `background: transparent`, `seed`, `text_guidance_scale}`; `extra` (outline, shading, detail, view, direction, isometric) merged first; answers `{usage, image}` at once | $0.013 (the dearest of PixelLab's $0.008–0.013 estimates; **(unverified)** curve) | adapter `pixflux_takes_the_palette_as_a_colour_image_and_answers_native_pixels`, `a_refusal_is_pixellabs_own_answer`; conformance `pixflux_forces_its_palette_at_native_size_and_recraft_takes_one_as_a_hint` | 🟡 wiremock |
| `recraft/*` generation rows | a hint: `controls.colors: [{rgb: [r, g, b]}]` (at most 10), inside any `controls` sent in `extra` | — | adapter `a_palette_is_sent_as_recrafts_preferred_colours` | 🟡 wiremock |

Not wired: Retro Diffusion (every job queued; `input_palette`, `check_cost`
behind `/v1/images/cost`), PixelLab's directions, rotations, animation and
tiles (8-direction characters are stored on PixelLab and fetched by id),
`max_frames` (no carried model animates a still), Ideogram's
`color_palette`. Not verified live: no PixelLab key.

### Preservation and background swap (3D doc §4, P2)

**`preserve_unmasked: true`** has litegen paste the init image's pixels back
outside the edit area — the mask, the region, or where they overlap, grown as
`mask_grow_px` grows it — before anything is stored, so those pixels come back
byte-identical from any vendor, whatever its own `preserves_unmasked`
promise. **`feather_px`** (0, a hard edge, by default; at most 256) blends from
the init at the area's edge to the vendor's pixels that far inside it
(chessboard distance), never outside. A vendor image of another size is
resized to the init's first; the result is PNG, or lossless WebP when webp is
asked. Every model taking a mask or region lists `supports_preserve_unmasked`,
and `GET /v1/models/{id}` serves it `mask_grow_px` (0–100), `preserve_unmasked`
and `feather_px` (0–256) in `litegen_params` (E11a: HTTP
`a_model_that_edits_part_of_an_image_serves_mask_growth_and_kept_pixels_with_their_caps`).
Refused before billing (`param_conflict`): no mask or region, `output_format`
jpeg, a `size` other than the init's, `feather_px` without
`preserve_unmasked`; a feather over 256 is `param_out_of_range`.

A **`background_swap`** model (its own `task`) takes a required init image and
a prompt for the new background (loader
`a_background_swap_model_takes_an_init_image_and_a_prompt`).

| row | behaviour | proof | status |
|---|---|---|---|
| core | composite, feather, resize | unit `outside_the_area_is_the_init_exactly_and_inside_is_the_vendors`, `a_feather_ramps_from_the_init_at_the_edge_to_the_vendor_inside`, `a_vendor_image_of_another_size_is_scaled_to_the_init_first` | ✅ |
| `mock/sloppy-edit-image` | fills the edit area #ffcd75 and adds 1 to the red of every other pixel, as a "similar" vendor does | HTTP `preserved_pixels_come_back_byte_identical_from_a_sloppy_vendor`, `preservation_the_request_cannot_honour_is_refused_before_billing` (no log); OpenAPI `preservation_documents_its_two_fields_and_the_catalog_flag` | ✅ mock |
| `mock/background-swap-image` | every exactly-#ff00ff pixel becomes #87ceeb, every other pixel stays | HTTP `a_background_swap_model_puts_the_subject_on_a_new_background` | ✅ mock |
| `recraft/replace-background` | `POST /v1/images/replaceBackground` multipart `image`, `prompt` (≤ 1,000), `model: recraftv3`, `response_format: b64_json`, `image_format` png (webp when asked), `negative_prompt`; `style` an extra; answers `data[0]`; $0.04 | adapter `replace_background_uploads_the_image_and_the_new_backgrounds_prompt`; conformance `recraft_replace_background_is_a_background_swap_at_the_raster_price` | 🟡 wiremock |

Ideogram, Bria and Stability (Replace Background and Relight, an async
endpoint) are §4's other swap vendors; none is wired.

### Per-box prompts and masks named in text (3D doc §4, P2)

**`layout: [{box: {x, y, w, h}, kind, prompt?, text?}]`** places objects
(`kind: object`, described by `prompt`) and words (`kind: text`, the `text`)
on a text-to-image canvas; boxes are fractions of the canvas, 0 to 1, since
the output size may not be known yet. Boxes guide the layout; no vendor clips
to them. The row's `layout` spec (listed as `supports_layout` and
`layout_kinds`) names the kinds, the most boxes, and whether a text box holds
one word. **`mask_prompt`** names the area an edit replaces instead of a
drawn mask (listed as `supports_mask_prompt`).

| row | vendor call | proof | status |
|---|---|---|---|
| `mock/layout-image` | a 256 px grey square, box i painted palette colour i where its fractions put it; object and text, at most 8 | HTTP `each_layout_box_is_painted_where_its_fractions_put_it`, `the_listing_says_which_models_take_a_layout_or_a_text_mask` | ✅ mock |
| `recraft/recraftv3`, `recraft/recraftv3_vector` | text boxes only, one word each, at most 32, as `text_layout: [{text, bbox: [[x,y],[x+w,y],[x+w,y+h],[x,y+h]]}]` (relative, clockwise from the top-left), written after `extra` (`text_layout` left the allowlist) | adapter `a_text_layout_is_sent_as_recrafts_four_point_boxes`; conformance `recraft_v3_lays_out_single_words_and_stability_replaces_a_named_area` | 🟡 wiremock |
| `mock/mask-prompt-image` | "finds" the middle half of the init image and fills it #ffcd75, every other pixel kept | HTTP `a_mask_named_in_text_edits_what_it_names` | ✅ mock |
| `stability/search-and-replace` | `POST /v2beta/stable-image/edit/search-and-replace` multipart `image`, `prompt` (required), `search_prompt` = `mask_prompt` (≤ 10,000), `grow_mask` 0 (its default 3 blurs the edge; `mask_grow_px` is not offered); $0.05 (5 credits) | adapter `search_and_replace_sends_the_named_area_as_search_prompt`; same conformance | 🟡 wiremock |

Refused before billing (HTTP `layouts_and_text_masks_a_model_cannot_take_are_refused_before_billing`,
no log; validator `a_mask_named_in_text_cannot_ride_with_a_drawn_one`): a box
outside the canvas or with no area, none or more than the row's boxes
(`param_out_of_range`); an object without a prompt or text without its text
(`param_required`); a kind the row lacks (`capability_unsupported`); several
words in a one-word text box (`param_out_of_range`); a layout beside an init
image, or a `mask_prompt` beside a mask or region (`param_conflict`); a
`mask_prompt` without an init image (`init_required`); either field on a row
without it (`param_unsupported`). The loader holds a layout to a
text-to-image row with kinds and a count (`a_layout_model_composes_text_to_image_boxes_of_kinds_it_names`).
BFL FLUX 3, Ideogram 4.x JSON prompts and Bria Fibo are §4's other layout
vendors, and Bria's by-text edits the other mask-prompt one; none is wired.

### Expanded canvases (3D doc §4, P2)

`expand: {left, top, right, bottom}` (pixels, each 0 or more, at least one
not) grows the init image's canvas; the result is the whole canvas with the
source at (`left`, `top`). Every image now reports its `width` and `height`,
read from its bytes — so the delivered canvas size is what the vendor sent,
never a promise. The row's `expand` spec (listed as `supports_expand`) says
the most pixels a side (`max_px`) and whether the vendor only centres
(`centered_only`: left = right and top = bottom, else 400
`expand_unsupported`). Refused before billing: no init image
(`init_required`), nothing added or a side over `max_px`
(`param_out_of_range`), a mask or region beside it (`param_conflict`), a
model without the spec (`param_unsupported`); the loader requires an `init`
role of a model that takes `expand`.

| row | vendor call | price | proof | status |
|---|---|---|---|---|
| `mock/expand-image` | the init exactly at (left, top), every new pixel #ffcd75; at most 512 px a side | $0 | HTTP `the_canvas_grows_by_each_side_with_the_source_at_left_top`, `an_expansion_the_model_cannot_make_is_refused_before_billing` (no log), `the_listing_says_which_models_expand`; validator `a_vendor_that_only_centres_takes_only_symmetric_expansions`; loader `an_expand_model_takes_the_init_image_it_grows` | ✅ mock |
| `stability/outpaint` | `POST /v2beta/stable-image/edit/outpaint` multipart `image`, `left`, `up` (top), `right`, `down` (bottom), each 0–2000; `prompt` only when given; `output_format`; `creativity` an extra | $0.04 (4 credits) | adapter `outpaint_sends_each_side_as_stabilitys_direction`; conformance `stability_outpaint_grows_each_side_by_up_to_2000_px` | 🟡 wiremock |

Kling's ratio-per-side expansion and Ideogram's centred reframe are not
wired (no Kling or Ideogram image route for them yet); BFL Expand, Recraft
and Bria are §4's other candidates. Not verified live: no Stability key.

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

### Background removal (L8)

A matting model is an edit on `POST /v1/images/generations`: one required
`init` image, no prompt, and the same image back as an RGBA PNG (or WebP,
where the row offers it) with its background transparent. Its row sets
`capabilities.background_removal`, listed as `supports_background_removal`;
the loader holds such a row to exactly that: an image `generate` model with
one required `init` role and nothing else, `prompt.max_length: 0`, none of
`text_to_image`, `image_to_image` or `inpainting`, and an `output_format`
(if any) of `png` and `webp` only. `content_type` is read from the bytes
(L9).

| row | vendor call | price | proof | status |
|---|---|---|---|---|
| `mock/remove-background` | every exactly-#ff00ff pixel made fully transparent, every other pixel as it came | $0 | HTTP `the_flat_background_comes_off_and_the_subject_stays_exactly`, `a_background_removal_model_takes_only_an_image_and_says_so_before_billing` (`ref_role_required`, `param_unsupported` for a prompt or `output_format`, no request log), `the_listing_says_which_models_remove_backgrounds` | ✅ |
| `fal/bria-rmbg-2` | `POST https://fal.run/fal-ai/bria/background/remove` `{image_url, sync_mode: true}` only — Bria's optional `prompt` is billed $0.03 instead of $0.018, so it is never sent; answers `image.url` (a data URI, or fetched with no key) | $0.018 per image | adapter `bria_rmbg_is_sent_only_the_image_and_its_cutout_comes_back` | 🟡 wiremock |
| `fal/birefnet-v2` | `POST …/fal-ai/birefnet/v2` `{image_url, sync_mode: true, output_format}` + allowlisted `model`, `operating_resolution`, `refine_foreground` (merged before `output_format`, which wins); `gif` is not offered (one-bit alpha) | **$0 (unverified)**: fal publishes "$0 per compute seconds" (`endpointBilling.price: 0`, 2026-10-30) | adapter `birefnet_gets_the_asked_container_and_its_own_knobs` | 🟡 wiremock |
| `recraft/remove-background` | `POST /v1/images/removeBackground` multipart `file`, `response_format: b64_json`, `image_format` png (Recraft's default is lossless WebP) or webp; answers one `image`, not `data[]`. The OpenAPI spec names the part `image`; the docs page and its examples say `file` **(unverified)** | $0.01 | adapter `remove_background_uploads_the_file_and_reads_the_one_image` | 🟡 wiremock |
| `stability/remove-background` | `POST /v2beta/stable-image/edit/remove-background` multipart `image` + `output_format` (png default) only — no prompt, strength or aspect ratio; the vendor refuses a side under 64 px or over 4,194,304 pixels (unbilled 400) | $0.05 (5 credits) | adapter `remove_background_sends_only_the_image_and_its_container` | 🟡 wiremock |

Conformance `background_removal_rows_restate_their_vendors_prices_and_containers`
restates each price and container list; loader
`a_background_removal_model_takes_one_init_image_and_nothing_else`; OpenAPI
`background_removal_is_a_catalog_flag_and_a_listed_capability`; TS
`background removal types`. Not verified live: no fal, Recraft or Stability
key is available here.

### Promptable segmentation (L7)

`POST /v1/images/segmentations` answers at once (no job, no stored row) with
masks for what `prompts` select in `image` (base64 or a `data:` URI; PNG,
JPEG, WebP or GIF). It serves image models whose `task` is `segment`, and
only those: either endpoint refuses the other's models with
`model_task_mismatch`. Point and box prompts together select **one** object
(a box around it, label-1 points on it, label-0 points left out); a text
prompt selects every instance of what it names, at most `max_masks`, and is
sent alone. Each mask comes back as litegen makes it from the vendor's file:
a grayscale PNG the image's size, white where selected (a vendor pixel is
selected when it is opaque and bright in any channel), with the box litegen
measured around the white and the vendor's score (null when it gives none).
Empty masks are left out; the rest are sorted best score first. Billed once
per request at the row's price.

| behaviour | rule | `fal/sam-3`, `fal/sam-3.1` | `mock/segment-image` | proof | status |
|---|---|---|---|---|---|
| request | `image` + `prompts` (`point {x, y, label 1\|0}`, `box {x, y, w, h}`, `text {text}`); prompt types ⊆ the row's `segmentation_prompts` | `image_url` data URI; points → `point_prompts [{x, y, label, object_id: 1}]`, boxes → `box_prompts [{x_min, y_min, x_max: x+w, y_max: y+h, object_id: 1}]`; text → `prompt` (else `""`: the schema's default is `"wheel"`), `return_multiple_masks` and `max_masks` 32; always `apply_mask: false`, `include_scores: true`, `sync_mode: true`, `output_format: png`; `POST https://fal.run/fal-ai/sam-3/image` (`…/sam-3-1/image`) | boxes exactly; the 32×32 square around each label-1 point, less those around label-0 points; text → two instances (the middles of the left and right halves) | adapter `sam_3_gets_points_and_boxes_as_one_object_and_returns_its_masks`, `a_text_prompt_asks_sam_3_for_every_instance`; HTTP `a_box_selects_exactly_the_box`, `points_together_select_one_object_a_square_around_each`, `a_text_prompt_finds_every_instance_best_first` | 🟡 wiremock / ✅ mock |
| response | `masks[{png_base64, score, box}]`, best first, empty masks left out, at most `max_masks`; boxes measured by litegen | masks as data URIs (or URLs, fetched with no key); `scores[i]`, else `metadata[{index, score}]`; fal's own `[cx, cy, w, h]` boxes are not used | scores 0.95 (points/boxes), 0.9 and 0.8 (text, sent worst first) | core `a_coloured_mask_on_transparency_becomes_white_on_black_with_its_box`, `a_mask_of_another_size_is_scaled_to_the_image`, `empty_masks_are_left_out_and_the_rest_come_best_first`; OpenAPI `segmentation_documents_its_prompts_its_masks_and_the_catalog_fields` | 🟡 wiremock / ✅ mock |
| refusals, before billing | 400: `param_required` (no prompts, empty text), `capability_unsupported` (a prompt type the row lacks), `param_out_of_range` (over `max_items`, two text prompts, label not 0/1, a box with no area), `param_conflict` (text mixed with points or boxes; only label-0 points), `prompt_out_of_bounds`, `image_unreadable`, `model_task_mismatch`, `model_media_type_mismatch`; 404 `model_not_found` | `max_items` 64 is litegen's (fal documents no cap) | `max_items` 16 | validator `segmentation_prompts_are_checked_against_the_model_before_billing`, `segmentation_prompts_must_fall_inside_the_image`, `a_segmentation_model_is_served_only_where_segmentations_are`; HTTP `a_request_the_model_cannot_serve_is_refused_before_anything_is_billed` (no request log) | ✅ |
| vendor errors | a vendor 4xx is `vendor_rejected` with fal's `detail[0].msg`; 5xx retried like an image | 422 → `RequestFailed {422, not retryable}` | — | adapter `sam_3_refusing_the_request_is_the_vendors_error` | 🟡 wiremock |
| billing | once per request | $0.005 (SAM 3), $0.01 (SAM 3.1) per request | $0.005 | HTTP `a_segmentation_is_charged_the_rows_price`; conformance `fal_sam_rows_take_all_three_prompt_types_and_at_most_32_masks` | ✅ |

Not verified live: no fal key is available here. Two choices rest on reading
fal's schema rather than a run **(unverified)**: that `prompt: ""` turns the
`"wheel"` default off for point/box requests, and that `apply_mask: false`
returns plain masks.

### Video

Every row here is capped at 🟡 by the family-wide validator gap noted above,
even where provider evidence is strong.

| param | kind | validator rule | # models | Proof | Status |
|---|---|---|---|---|---|
| `duration_seconds` | Float (39 models) / Int (4 models) | range | 43 | validator: none (`validate_video` untested). provider: `runway::tests::submits_runway_job_and_returns_handle` (`body["duration"]==5`) — **note the outbound key is `duration`, not `duration_seconds`; this row is exactly the kind of silent rename this doc exists to surface**. conformance: `vidu_q1_duration_and_resolution_match_the_model_map`, `bedrock_nova_reel_duration_matches_the_text_video_task`. | 🟡 |
| `resolution` | String enum | enum | 31 | validator: none. provider: `google::tests::submits_veo_job_and_sends_api_key_header` (`body["parameters"]["resolution"]=="720p"`), `leonardo::tests::submits_image_to_video_and_polls` (`body["resolution"]=="RESOLUTION_720"`). conformance: `vidu_q1_duration_and_resolution_match_the_model_map`. | 🟡 |
| `aspect_ratio` | AspectRatio | enum `allowed` | 26 | validator: none. provider: setters exist in `kling.rs`, `luma.rs`, `pixverse.rs`, `vidu.rs`, `fal.rs`; the only *assertion* found is a negative case, `fal::tests::*` (`body.get("aspect_ratio").is_none()` for ltx-video, which doesn't support it) — **no test proves the correct value reaches the body for a model that does support it**. conformance: `runway_video_aspect_ratios_are_in_the_gen4_enum`. | 🟡 |
| `seed` | Seed | range | 13 | validator: none. provider: setters exist in `bedrock.rs`, `vidu.rs`, `pixverse.rs`, `runway.rs`, `fal.rs`, `replicate.rs` — **zero assertions on any of them**; a regression that dropped or mis-mapped video seed would go unnoticed by the suite. conformance: n/a. | 🟡 |
| `negative_prompt` | String | max_length | 4 | validator: none. provider: the setter is in `kling.rs` (PixVerse's endpoints have no such field, and its dead write is gone) — **zero assertions** on the video value. conformance: n/a. | 🟡 |
| `fps` | Int | range | 4 | validator: none. provider: `bedrock.rs` sets `video_config["fps"]` — **zero assertions anywhere in the suite**. conformance: n/a. | 🟡 |

### Video operations (3D doc §5, P2)

`extend`, `retake` and `video_edit` models (their `task`) work on a
**`source_video`** — `{type: generation | url | base64, value, format?}`, as
`source_model` — which litegen reads and measures before anything is
reserved: a generation after the same access check `GET /v1/generations/{id}`
makes (finished, with a stored video), a URL fetched as a public one, or
base64 with its `format`. Its row's `params.source_video` lists the containers
it takes, its longest source, whether it chains only its own vendor's
generations (`own_generations_only`, refused otherwise with
`source_not_chainable`; Luma and Kling work so) and, for an extend model, its
`extend_output` (`full` | `added`). litegen splices nothing: the finished job
reports **`segments: [{start_s, end_s, origin: source | generated}]`**, the
plan recorded at submit from the request, the source's length and the row's
`extend_output`, on the live poll and the row answer alike.

| task | request | mock | proof | status |
|---|---|---|---|---|
| `extend` | `source_video`, `direction` (`forward` \| `backward`), `duration_seconds` (the length to add), `prompt` | `mock/extend-video`: GIF sources, the whole clip back, every source frame copied byte-for-byte (the `gif` crate, no re-quantising) | HTTP `extending_forward_keeps_the_source_frames_first_and_says_so`, `extending_backward_puts_the_new_frames_first` | ✅ mock |
| `retake` | `source_video`, `time_range: {start_s, end_s}` (required), `replace` (`video` \| `audio` \| `both`), `prompt`; the row's `time_range` spec sets `min_window_s` (LTX's 2 s) and `whole_seconds` (Runway) | `mock/retake-video`: regenerates every frame starting in the window (at least 0.5 s), copies the rest | HTTP `a_retake_regenerates_only_its_window`; validator `a_window_off_a_whole_second_grid_is_refused_where_the_model_keeps_one` | ✅ mock |
| `video_edit` | `source_video`, `prompt`, `reference_images`, `time_range` where the model takes one; strength stays in `extra` | — (segments: the window, or the whole clip, generated) | loader `every_video_operation_names_its_source_and_only_those_do` | ✅ catalog |

Refused before billing (HTTP `an_operation_the_source_or_model_cannot_serve_is_refused_before_billing`,
no log): no `source_video` on an operation (`source_video_required`), a value
outside `direction`/`replace` or a container the row lacks
(`param_enum_mismatch`), a retake without a window (`param_required`), a window
too short, off-grid, not a window or past the source's end
(`param_out_of_range`), a source the caller cannot read (404
`source_not_found`), unfinished (409 `source_not_ready`), unreadable or too
long (422), `source_video` on a plain generation (`param_unsupported`). The
loader requires `params.source_video` of every video task but `generate` (and
refuses it there), `extend_output` of an extend and `time_range` of a retake.
OpenAPI `video_operations_document_their_source_window_and_segments`; SDKs TS
`video operation types`, Python `TimeRange` and `VideoSegment` mirrored.
Vendors (LTX, Luma `ray-3.2`, Runway `aleph2`, Veo 3.1, PixVerse v6, Seedance
2.x, Wan 2.7) are §5's candidates; none is wired.

### Video controls for sprites (L10)

`camera` (`fixed` | `auto`), `audio` (bool) and `loop` (bool) are request
fields where a row declares them (strict refuses, lax drops and reports an
undeclared one); the listing serves `supports_fixed_camera`,
`supports_audio_toggle`, `supports_loop` and `duration_values` — a
`duration_seconds` param may list the only values its vendor takes, and any
other is 400 `param_enum_mismatch`. Every finished video reports `media
{duration_s, fps, width, height, has_audio}`, read from the kept bytes (MP4/MOV
boxes or GIF frames) and recorded on its asset.

| control | rows | wire | proof | status |
|---|---|---|---|---|
| `camera: fixed` | `bytedance/doubao-seedance-1-0-pro-250528`, `seedance-1-0-pro-fast-251015` | `camera_fixed: true` | `submits_seedance_task_and_polls` | 🟡 wiremock |
| `audio` | `bytedance/dreamina-seedance-2-0-mini-260615` (`generate_audio`, default on), `fal/ltx-2.3`, `fal/flux-3-video`, `-video-draft`, `-max-video` (`generate_audio`, on), `kling/video-kling-v2-6`, `-v3` (`sound` on/off, off), `pixverse/v6`, `c1` (`generate_audio_switch`, off), `vidu/viduq3-turbo`, `-pro` (`audio`, on) | as listed | `submits_fal_video_job_and_returns_handle`, `submits_text2video_with_jwt_and_polls`, `submits_text_to_video_and_polls`, `submits_text2video_with_token_auth_and_polls` | 🟡 wiremock |
| `loop` | `luma/dream-machine`, `ray-2`, `ray-flash-2` | `loop`; refused with a last frame ("Loop is not supported for keyframes") | `submits_luma_job_with_first_frame_keyframe`, `what_luma_cannot_make_is_refused_before_any_call`; conformance `every_luma_video_row_takes_loop` | 🟡 wiremock |
| `duration_values` | Kling 2.5 Turbo / 2.6 (5, 10), MiniMax Hailuo 02 / 2.3 / 2.3 Fast (6, 10), fal LTX-2.3 (6, 8, 10), Leonardo Kling 2.5 (5, 10), PixVerse v3.5 / v4.5 / v5 (5, 8), Luma Dream Machine / Ray 2 / Ray Flash 2 (5, 9), Hunyuan Video / Video I2V (5 only, as a 5–5 range) | the validator refuses others | HTTP `controls_and_durations_a_model_lacks_are_refused` | ✅ |
| `media` | every finished video | — | unit `an_mp4_says_its_length_rate_size_and_audio`; HTTP `a_looping_still_silent_clip_reports_what_came_back` (`mock/sprite-video`: 64×64, 10 fps, 2 s, silent, last frame = first) | ✅ |

Kling's audio prices (Task 17) now read the canonical `audio`, as do
PixVerse's.

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
| `joint_naming` | `string` (`Model3dJointNaming`) | `spec` (`vrm`, `mixamo`, `unity`, `unreal`; `vendor` sends none); refused on a non-humanoid rig before any call | not sent: `vrm`, `unity`, `mixamo` are litegen's renames of Meshy's names (also on `meshy/rig`) | `vrm` emits VRM 1.0 bone names itself; `unity`, `mixamo` are litegen's renames | `a_humanoid_rig_asks_for_v3_and_the_requested_naming`, `joint_naming_on_a_creature_is_refused_before_any_call`; HTTP `vrm_joint_naming_delivers_vrm_bone_names`, `unity_and_mixamo_naming_are_litegens_renames_of_the_vendors_joints`; unit `a_rig_litegen_can_read_is_renamed_bone_for_bone_into_another_scheme`; validator `a_humanoid_naming_scheme_on_another_skeleton_is_refused` | 🟡 wiremock / ✅ mock |
| `height_m` | `float` | — | `height_meters` (any positive value) | scales the humanoid (0.5–3 m) | `meshy_rigging_sends_the_source_as_a_glb_data_uri_and_the_height`, `meshy_rigging_without_a_height_leaves_merely_the_source`; HTTP `height_m_sets_the_rigged_characters_height`; validator `a_height_outside_the_rigs_range_is_refused` | 🟡 wiremock / ✅ mock |
| `output_formats` | `string_array` | `out_format`: `fbx` when fbx is promised, else `glb` | the endpoint returns both; glb is downloaded, Meshy's own fbx too when promised | glb | `an_fbx_promise_asks_tripo_for_its_own_fbx`, `a_finished_rig_downloads_the_promised_containers` | 🟡 wiremock |
| preflight | — | `POST /v3/animations/rig-check` (free), polled at `GET /v3/tasks/{id}`: `riggable: false`, a failed check, or a `rig_type` other than the requested skeleton → 422 `not_riggable` | none | a flat source (thinnest side < 1 % of longest) → 422 `not_riggable` | `a_source_rig_check_refuses_is_not_riggable`, `a_skeleton_rig_check_did_not_find_is_not_riggable`; HTTP `a_source_the_vendor_cannot_rig_is_refused_before_anything_is_billed` (no row, no log) | 🟡 wiremock / ✅ mock |
| manifest `skin` | — | measured from the delivered file | measured | `the_mock_rig_reports_its_skeleton_exactly` (§3I's joints and humanoid map) | `a_rig_no_table_knows_reports_the_requested_skeleton_and_no_humanoid_map` | ✅ |

**litegen's renames (3D doc §3B, P2).** Where the vendor cannot name joints
in the asked scheme, delivery renames a humanoid rig it can read: it reads
the joints with its humanoid tables (VRM, Unity, Mixamo, Meshy, the mock),
then renames every bone that the asked scheme's table names — `vrm`, `unity`,
or `mixamo` (with its `mixamorig:` namespace); other joints keep their names,
nothing else changes, and the vendor's own file is kept as `original`. A rig
already named so (the vendor did it) is left alone; a rig no table reads
keeps the vendor's names, with a delivery note. `unreal` has no table (no
official UE5 Manny list), so it is offered only where the vendor names it
(Tripo's `spec`). Every scheme but `vendor` names humanoid bones: with a
non-humanoid `skeleton` (asked or the row's default) it is 400
`param_conflict`.

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
than were asked); a clip that leaves `name` out (or sends null) is named by the
validator before anything is quoted or stored (E11c): after its canonical
preset, else its preset id with every other character as `_`, a prompt clip
`clip-<n>` by its place, `-2`, `-3`, … on a taken name. It holds `in_place`
clips the vendor did not, keys every clip at
`fps` when the vendor did not, and measures each clip into the manifest from
its keys. An animate model returns its source's geometry, so its catalog row
says `native_orientation: {up: source, front: source}` and the delivered front
is the source manifest's.

| litegen param | ParamSpec | `tripo/animate` | `meshy/animate` | `mock/animate-3d` | proof | status |
|---|---|---|---|---|---|---|
| `animations[].preset` | `animation_list` | `animations` (≤ 5; the 16 v2.5/v3.0 preset ids, 10 tagged canonical) | `action_ids` (≤ 10, no repeats — refused before the call; 678 library ids with Meshy's names, 12 tagged canonical) | `mock:*` (3 canonical) | validator `a_clip_names_a_canonical_preset_or_one_of_the_models_own`; tripo `a_retarget_sends_the_rigs_task_id_its_presets_and_tripos_in_place_flag`; meshy `an_animation_goes_to_the_meshy_rig_with_its_action_ids_and_change_fps`, `a_repeated_action_is_refused_before_any_call`; conformance `tripo_animate_offers_the_retarget_presets_on_tripos_own_rig`, `meshy_rig_and_animate_take_what_meshys_endpoints_accept` | 🟡 wiremock / ✅ mock |
| `animations[].name` | — (optional: litegen names an unnamed clip) | renamed by litegen in order | renamed by litegen | renamed by litegen | validator `clip_names_are_unique_and_well_formed`, `an_unnamed_clip_is_named_after_its_preset_or_its_place`, `a_typed_name_is_kept_and_an_unnamed_clip_steps_around_it`, `an_unnamed_clip_whose_preset_gives_no_name_is_named_by_its_place`, `a_clips_refusal_names_its_place_in_the_list`; HTTP `an_unnamed_clip_is_delivered_under_the_name_litegen_gives_it`; delivery `a_pose_clip_beside_the_asked_ones_is_dropped_and_the_rest_named_in_order`, `a_vendor_returning_fewer_clips_than_asked_fails_the_delivery`; HTTP `one_clip_per_name_in_request_order_kept_in_place_by_the_vendor` | ✅ |
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

### Texture, remesh, segment (3D doc §3F)

Operations on a mesh, submitted to `POST /v1/models3d/generations` like any
3D job: `source_model` names the mesh (a generation id, a URL or base64) and
is checked against the row's `params.source_model` before billing (§3A). Each
keeps its source's axes (`native_orientation: source`).

| task | request | delivered and measured | mock | proof | status |
|---|---|---|---|---|---|
| `texture` | `source_model`; `prompt` and/or a `style` reference image (neither: 400 `param_required`); `pbr`; `texture_resolution`, an int enum of the row's `values` (listed as `texture_resolutions`; anything else `param_enum_mismatch`; a row must list them, loader); `keep_uvs` (listed as `supports_keep_uvs`) | `materials[].maps` names the maps; `uv_sets` | `mock/texture-3d`: one material — `base_color`, plus `metallic_roughness` and `normal` with `pbr` — each `texture_resolution` px square; planar UVs unless `keep_uvs` | HTTP `texturing_paints_the_source_and_the_manifest_names_its_maps` (reads the delivered map's size), `a_texture_request_says_what_to_paint_and_asks_a_size_the_model_makes` (no log); loader `a_texture_resolution_lists_the_sizes_the_model_paints_at` | ✅ mock |
| `remesh` | `source_model`, `target_polycount`, `topology` (`triangle` \| `quad`) | `triangle_count` | `mock/remesh-3d`: a ring of exactly `target_polycount` triangles, or `target_polycount / 2` quads (two triangles each in glTF) | HTTP `remeshing_delivers_the_asked_triangle_count` | ✅ mock |
| `segment` | `source_model` | one glTF node per part, named with the vendor's label or `part_<n>` (n: its place among the mesh-bearing nodes); the manifest's `parts: [{name, node, triangle_count, bounds}]`, absent on every other model's mesh | `mock/segment-3d`: triangles split by their centre along X, the −X half labelled `negative_x`, the +X half unlabelled (delivered `part_1`) | HTTP `segmenting_splits_the_source_into_named_parts`; unit `unnamed_parts_are_named_by_their_place_and_labels_are_kept`, `each_mesh_node_is_a_part_with_its_own_triangles_and_world_bounds` | ✅ mock |

Conformance `texture_remesh_and_segment_models_take_what_they_work_from_and_keep_its_axes`;
OpenAPI `texture_remesh_and_segment_document_their_params_and_the_parts`;
SDKs: TS `3D operation types`, Python `ScenePart` mirrored and
`Model3dGenerateParams` covering the new fields. Vendors: none wired yet —
§3H's candidates are Meshy `/openapi/v1/retexture` and `/openapi/v1/remesh`,
Tripo `/v3/models/texture`, `/v3/mesh/decimate` and `/v3/mesh/segment`, and
Tencent's texture edit, smart topology and HY 3D Part. ⬜ vendor.

### Opt-in placement (3D doc §3D)

`ground: true` (lowest point at y = 0), `center: true` (bounds centred on x
and z) and `height_m` (that tall at rest) are request fields on every 3D
model, listed as `supports_placement`: litegen applies them on delivery,
after orientation, as one new root node (`litegen_placement`) over the whole
scene — a uniform scale about the origin, then the move — so skins, clips and
the vendor's own file (kept as `original`) are untouched. A rig model that
declares `height_m` still sends it to its vendor too; elsewhere `height_m`
must be above 0 and at most 10,000 m (`param_out_of_range`).
`GET /v1/models/{id}` serves the three in `litegen_params` (E11a): `ground`
and `center` as `bool`, `height_m` as `float` with `max: 10000` and no `min`
(above 0 is exclusive). A rig that declares its own `height_m` keeps it in
`params` alone, served with litegen's 10,000 m ceiling when it states none,
since the validator holds it to that.

| field | rule | proof | status |
|---|---|---|---|
| `ground`, `center`, `height_m` | one root transform; bounds measured at rest (a skinned mesh's bind pose); a mesh already placed is left alone | unit `placement_scales_grounds_and_centres_through_one_new_root_and_leaves_skins_and_clips_alone`; HTTP `ground_center_and_height_place_the_mesh_and_keep_the_vendors_file`, `height_m_scales_a_rigged_animated_mesh_without_touching_its_skin_or_clips`, `placement_is_offered_on_every_3d_model_and_a_height_must_be_one`, `every_3d_model_serves_its_placement_fields_with_the_bounds_the_validator_keeps`, `a_declared_height_without_a_ceiling_is_served_with_the_ceiling_the_validator_keeps`; validator `every_3d_param_is_refused_in_strict_and_dropped_in_lax_where_undeclared` (height_m passes as placement); loader `litegen_params_cannot_be_set_from_yaml`; OpenAPI `placement_is_three_request_fields_and_a_listed_capability`, `the_served_schema_documents_the_fields_litegen_applies` | ✅ |

### Normalised delivery (3D doc §2 rule 3, §3D)

One step, `proxy::model3d_deliver::deliver`, runs on a finished job's files at
whichever observer sees completion first (the client's poll or the poller),
from one plan fixed at submit (`metadata.delivery`). It maps no vendor field;
it is what every vendor's mesh goes through before litegen stores it.

| behaviour | rule | proof | status |
|---|---|---|---|
| read | The vendor's mesh is read with litegen's own reader. One it cannot read (a required extension the reader doesn't implement, a corrupt file, a Draco stream past the reader's limits; Draco and meshopt themselves are read since mesh-gaps item 2, 2026-11-06, and `KHR_mesh_quantization` since 2026-10-12) fails the generation, naming it and any promised container that cannot be derived (P-R7). | `a_mesh_litegen_cannot_read_fails_the_delivery_and_names_what_is_missing`; HTTP `a_mesh_litegen_cannot_read_is_never_delivered`; `a_provider_that_under_delivers_an_underivable_format_fails_the_generation` | ✅ |
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
  the converter cannot read — a glb requiring an extension the reader doesn't
  implement, say; Draco and meshopt are read since mesh-gaps item 2 — because
  nothing is then derivable from it. `mock/partial-3d` is exactly that case, and the
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
| price | $0.30 textured (default); $0.20 untextured, applied by `variable_pricing` | $0.375; +$0.15 each for views, PBR and a face count, applied by `variable_pricing` | [Prices that match the bill](#prices-that-match-the-bill-3d-doc-3g-l14): quote = reservation = charge; conformance `every_base_price_is_what_the_rules_charge_for_the_default_request` | 🟡 fal quotes |

**`fal/hyper3d-rodin`** takes views too. Its `condition_mode` `concat` (the
default, left an `extra`) takes "multiple multi-view images of the same object
… in any order, regardless of the order of view", so every `ImageView` is a
role, at most five references in all (its `max_total`), sent as one
`input_image_urls` list front first and the rest by yaw. No view order is
declared, since the vendor reads none. Price as for an image request ($0.40;
HighPack ×3). Proof: adapter
`rodin_takes_views_as_one_unlabelled_list_front_first`; tie
`each_catalog_row_declares_exactly_what_its_endpoint_takes`. Status 🟡 wiremock.

Not verified live: no fal key is available here, so no vendor's "left" has
been proven with an asymmetric fixture yet (Rodin's views are unlabelled, so
its "left" cannot be wrong).

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
| ref roles `view-*` | multi-image-to-3d | multiview-to-model (on fal: `fal/tripo-h3.1-multiview`, above) | — | `condition_mode` (on fal: `fal/hyper3d-rodin`, above) | ⬜ |

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

## Final review fix pass (Task 44, 2026-10-31)

The whole-branch review's Critical and Important findings, each fixed with a
test that failed first. Rows above that these change say so; the rest are
contract fixes that touch no vendor field.

| finding | what changed | test (RED → GREEN) |
|---|---|---|
| uncapped downloads (source_model URL, source_video URL, edit init/mask URL, re-hosted video) | one streamed, capped reader (`util::ssrf::download_capped`) behind the public-address check and the no-redirect client; a URL `source_video` is capped at 256 MiB | `a_body_past_the_cap_is_refused_while_it_streams`, `a_download_reports_its_status_or_returns_the_body_and_its_type` |
| `Idempotency-Key` stranded by a hang-up | the keyed request runs and is recorded in a task of its own | `a_client_that_hangs_up_mid_call_gets_the_recorded_outcome_on_retry` |
| a free preflight, or a request no deployment was asked, spent the key | `PreflightFailed`, and `AllDeploymentsFailed`, never count as a vendor call | `a_failed_preflight_or_an_unattempted_request_never_reached_a_billed_call` |
| `retryable` wrong without an HTTP status | the adapter's verdict when there is no status; never once the vendor accepted the job (504 included) | `a_moderated_job_says_not_to_retry` |
| timed-out billed calls re-sent (10 adapters) | `providers::may_resend_after(err, billed)`: only a call that never connected is re-sent; a poll may be | `a_timed_out_billed_call_is_never_sent_again_but_a_read_may_be`, `no_adapter_decides_for_itself_that_a_timed_out_call_may_be_sent_again` |
| Ideogram re-generated on a failed download | `fetch_result` (public, capped, never re-sent), status before body | `a_result_that_will_not_download_is_never_generated_again`, `a_gateway_page_or_a_rate_limit_may_be_sent_again` |
| Vidu / BytePlus / MiniMax parsed before the status (W14), MiniMax 1002, V2 failed progress (W15) | `providers::read_vendor_json`; "`1002`, Rate limit triggered" may be sent again; a terminal poll reports 100 | `a_gateway_page_or_a_rate_limit_may_be_sent_again` (×5), `a_turned_away_v1_submit_may_be_sent_again`, `h3_poll_maps_the_v2_task_statuses` |
| fractional durations billed then truncated (C5) | an `int` duration is rounded once, in the request, for price and vendor alike; Vidu rows are `int` | `a_whole_second_duration_is_rounded_for_the_price_and_the_vendor_alike`, `every_vidu_row_takes_whole_seconds` |
| Kling face reference replaced | the caller's `extra.image_reference` is sent; `subject` only as a subject reference's default | `a_face_reference_the_caller_names_is_sent_as_one` |
| reservation and limits without the markup | the handlers reserve the marked-up price the router charges | `a_reservation_holds_the_marked_up_price_it_will_charge` |
| OpenAI quoted and charged at 1024² medium | `variable_pricing` by quality and `multiply_per_megapixel_above: 1.048576` | `a_megapixel_multiplier_scales_an_image_larger_than_its_anchor`, `an_openai_image_is_quoted_by_its_quality_and_its_size` |
| PixVerse Multi-Transition open-ended | unlisted lengths bill the next listed tier up; `max_time_s: 30` on the keyframe role | `a_multi_transition_clip_of_an_unlisted_length_bills_the_next_tier_up` |
| limits refund below zero | `spend_usd = MAX/GREATEST(spend_usd + ?, 0)` | `sqlite_management_store` (and the Postgres twin) |
| unrecorded jobs billed and left running | settled to 0, logged failed; a video job is forgotten and cancelled at the vendor | `an_unrecorded_video_job_is_logged_failed_and_let_go`, `an_unrecorded_3d_job_is_logged_failed` |
| cross-tenant video read on a DB error | the row is read once; an error is a 500 | `a_video_is_not_readable_across_tenants_when_its_row_cannot_be_read` |
| image cache key | every serialised image option, plus a mask litegen drew or grew | `every_field_that_changes_the_image_changes_its_cache_key` |
| `catalog_version` differed per process | fingerprint over a key-sorted form | `the_catalog_version_is_a_function_of_the_catalog_alone` |
| `code: 500` | `"internal_error"` | `an_internal_error_names_its_code_as_a_string` |
| `supports_animation` missing; rule-5 flags | `supports_animation`, `supports_skeleton`, `supports_joint_naming`, `supports_mask_grow` | `the_listing_says_which_models_animate_and_what_a_rig_takes`, `the_listing_says_how_each_model_edits_part_of_an_image` |
| `fal/meshy-rig` refused `skeleton: humanoid` | `skeleton: [humanoid]` | `fal_meshy_rig_takes_a_textured_glb_within_meshys_limits` |
| a stage started inside a GET could be stranded; poll contexts untested | claim, billed start and record run as one task | `a_stage_a_hung_up_poll_was_starting_is_recorded_all_the_same`, `a_context_a_poll_hands_back_reaches_the_row` |
| Tripo retarget asked for Tripo's fbx | always glb; the fbx is derived | `a_retarget_is_asked_for_tripos_glb_even_when_an_fbx_is_promised` |
| delivery notes only in the log; WebP named KTX2; fps resample untested | `SceneManifest.notes`: a texture left out (named by its format), a naming the rig could not take, what a derived container lost | `what_delivery_could_not_do_is_named_on_the_asset`, `clips_a_vendor_keyed_at_another_rate_are_delivered_at_the_requested_fps` |
| published references | only sniffed images, typed by their bytes, at a public `https` origin (CGNAT refused); `nosniff` and `sandbox` on the refs and asset routes | `only_an_image_is_published_and_it_is_typed_by_its_bytes`, `a_reference_is_published_only_at_a_public_https_origin`, `a_url_only_vendor_fetches_the_upload_from_an_unguessable_litegen_url` |
| asset route served any key; sweeper swept the whole directory | litegen's own result prefixes only (never `litegen/refs`); the sweep walks only `litegen/` | `only_litegens_own_result_files_are_served_from_the_asset_route`, `the_sweeper_leaves_whatever_is_not_litegens_alone` |
| the image could not write its asset directory | `/app/data` created and owned by 1000:1000, a volume, `LITEGEN__IMAGE_STORAGE__LOCAL_DIR`; startup refuses an unwritable directory | `an_asset_directory_litegen_cannot_write_is_named_at_startup`; a trixie-slim probe image as 1000:1000 (before: `Permission denied`; after: written) |
| `source_not_chainable` half built; `video_edit` had no mock | own-generations-only rows refuse a URL or upload before fetching, and compare the vendor account recorded on video rows; `mock/edit-video` | `a_model_that_edits_only_its_own_generations_refuses_any_other_source`, `a_whole_clip_edit_regenerates_every_frame_of_its_source` |
| slow mock timed out only under 30 s | answers after a day | `the_slow_mock_outlasts_any_vendor_timeout` |
| Playground: URL source format, segmentation by URL, history quota, placement | the shown format is sent; segmentation is upload-only; history drops inline images and masks and never fails a run; `ground` / `center` / `height_m` on every 3D model, as `litegen_params` serves them | vitest `sends a URL source with the format its select shows…`, `builds a segmentation request…`, `what a history entry keeps`, `a history write that cannot be stored`, `takes each field and its bounds from litegen_params` |

## Vendor feature audit (2026-11-03)

Every carried row's declared features against its vendor's current schema, in
both directions, and each adapter's mapping of every unified feature. Ledger:
`.superpowers/sdd/2026-11-03-vendor-audit/progress.md` (git-ignored). Each cell
below has a test that failed before the fix unless marked *guard*.

| param × provider | what changed | proof | status |
|---|---|---|---|
| `aspect_ratio` × Recraft (8 generation rows) | declared (the Appendix's 14 ratios), sent as `size: "w:h"`; an exact size wins | `an_aspect_ratio_goes_as_size_and_an_exact_size_wins`, `every_recraft_generation_model_takes_the_appendix_aspect_ratios` | 🟡 wiremock |
| `size` × Seedream 4.0 / 4.5 / 5.0 lite | min/max pixels and `max_aspect: 16` (the vendor's pixel rule) | `bytedance_seedream_sizes_carry_the_vendors_pixel_range_and_ratio` | ✅ |
| `output_format` × Seedream 5.0 lite | png, jpeg; label from the bytes | `the_output_format_goes_as_its_own_field_and_labels_the_image` | 🟡 wiremock |
| `seed` × Seedream | never sent (no model we carry has one) | `a_seed_never_reaches_seedream` | 🟡 wiremock |
| `duration_seconds` × Seedance (3), PixVerse v6/c1 | `kind: int` (C5) | the five duration facts in `catalog_conformance` | ✅ |
| `duration_seconds` × Leonardo Veo 3.1 / Fast | `values: [4, 6, 8]` | `leonardo_veo3_1_*_contract` | ✅ |
| `duration_seconds` × vidu2.0 | `values: [4, 8]`; unpriced pairs refused, never snapped | `vidu2_makes_only_the_lengths_and_sizes_vidu_prices` | 🟡 wiremock |
| `duration_seconds` × PixVerse v3.5 | 8 s at 1080p refused | `v3_5_makes_no_8_s_clip_at_1080p` | 🟡 wiremock |
| `resolution` × Kling video (3 rows) | declared; on Kling's current API sent as `settings.resolution` | `resolution_and_multi_shot_go_in_settings` | 🟡 wiremock |
| Kling video × the current API (3 rows) | `POST /text-to-video/<model>` / `/image-to-video/<model>` (`prompt` or `contents`, `settings`), `GET /tasks`, API key; no `negative_prompt` | `text_to_video_goes_to_the_models_path_with_the_api_key_and_polls_tasks`, `image_to_video_sends_contents_with_the_first_frame_first`, `every_documented_status_maps`, `a_legacy_handle_still_polls_on_the_legacy_path` | 🟡 wiremock |
| refs × Gemini 2.5 Flash Image | 3 input images | `gemini_2_5_flash_image_takes_three_input_images` | ✅ |
| `seed` × Gemini image rows | not declared (Ruling) | `gemini_image_rows_declare_no_seed` (*guard*) | ✅ |
| refs × FLUX.2 pro / max / klein | 8 / 8 / 4, each to its own `input_image_N` | `every_reference_goes_to_its_own_input_image` | 🟡 wiremock |
| `steps`, `guidance_scale` × BFL flux-dev | declared; `steps`, `guidance` | `flux_dev_sends_steps_and_guidance_as_its_own_fields` | 🟡 wiremock |
| `redux` role × BFL flux-dev, Ultra | `image_prompt` (+ Ultra's `image_prompt_strength` extra) | `an_ultra_image_prompt_goes_as_image_prompt` | 🟡 wiremock |
| `style` × Stability (8 more rows) | `style_preset` on every route that takes it, v1 SDXL included | `sdxl_sends_style_as_style_preset_on_every_v1_route` | 🟡 wiremock |
| `guidance_scale` × Stability sd3 rows | `cfg_scale`, sd3 route only | `guidance_goes_as_cfg_scale_and_only_to_the_sd3_route` | 🟡 wiremock |
| init + `strength` × Stability Ultra, sd3-turbo | declared (the adapter already sent them) | `stability_ultra_and_sd3_turbo_take_an_image_to_start_from` | ✅ |
| prompt / `negative_prompt` / `output_format` × Stability inpaint | 10,000-char caps; jpeg/png/webp | `stability_inpaint_carries_its_prompt_limits_and_output_format` | ✅ |
| `size` × Ideogram 3.0 | the 69 ResolutionV3 sizes; an exact size drops the ratio | `a_3_0_size_goes_as_resolution_without_an_aspect_ratio` | 🟡 wiremock |
| `reference` role × Ideogram 4.5 Precise Edit | ≤ 4, 3 beside a mask | `a_masked_precise_edit_takes_at_most_three_references` | 🟡 wiremock |
| `size`, `seed` × MiniMax image-01 | width/height 512–2048 ×8 (no ratio beside); seed | `a_size_goes_as_width_and_height_and_a_seed_goes_too` | 🟡 wiremock |
| `seed` × Vidu (6 rows) | 1–2147483647 (0 is Vidu's random) | `a_seed_goes_as_vidus_seed` | 🟡 wiremock |
| `audio` × Vidu viduq1, viduq2-pro | only where the endpoint has the switch; viduq1's reference2video `images` form has none (2026-11-09), so `audio: true` there is refused | `audio_goes_only_where_the_endpoint_has_a_switch`, `viduq1_reference_images_send_no_audio_switch` | 🟡 wiremock |
| `seed` × Hunyuan image | sent with `Revise: 0`; refused beside a reference or Revise 1 | `a_seed_turns_prompt_expansion_off_so_it_takes_effect` | 🟡 wiremock |
| `size` × Hunyuan image with a reference | the four sizes Tencent allows | `a_reference_image_takes_only_the_four_sizes_tencent_lists` | 🟡 wiremock |
| `reference` role × OpenAI GPT image (3 rows) | ≤ 15 beside `init`, sent as `image[]`, edited image first | `several_images_go_as_an_image_array_with_the_edited_one_first` | 🟡 wiremock |
| `guidance_scale`, `steps` × Leonardo Diffusion XL | `guidance_scale`, `num_inference_steps` | `guidance_and_steps_go_as_leonardos_own_fields` | 🟡 wiremock |
| `seed`, `negative_prompt` × Leonardo Motion 2.0 | `seed`, `negativePrompt` | `motion2_sends_its_seed_and_negative_prompt` | 🟡 wiremock |
| `strength` × Replicate flux-dev, SDXL | declared (the adapter already sent `prompt_strength`) | `replicate_img2img_rows_take_a_strength` | ✅ |
| `docs_url` × every row | whitespace refused by the loader (4 rows had swallowed a description line) | `rejects_a_docs_url_that_swallowed_the_next_line` | ✅ |


## New models (2026-11-03)

Models added on 2026-11-03 from the vendors' current API pages, each with its
facts in `catalog_conformance` (test first) and its wire shape in the adapter's
wiremock tests. Ledger: `.superpowers/sdd/2026-11-03-kling-new-api-and-new-models/progress.md`
(git-ignored). None was run live.

| model × param | wire | proof | status |
|---|---|---|---|
| `kling/video-kling-v3-turbo`, `-v4` × route | `POST /text-to-video/kling-3.0-turbo` / `kling-4.0` (and `/image-to-video/…`), `settings`, `contents` | `the_new_models_go_to_their_own_paths`, `every_kling_video_row_has_a_path_model` | 🟡 wiremock |
| Kling 3.0 Turbo / 4.0 × `duration_seconds`, `resolution`, `aspect_ratio`, frames | int 3–15 s / 3–30 s; 720p, 1080p; 4.0 adds 21:9 and a last frame | `kling_3_0_turbo_and_4_0_are_carried_with_their_documented_contract` | ✅ |
| `bytedance/dola-seedream-5-0-pro-260628`, `-flash-260915` × `size`, `output_format`, refs | 921,600–4,624,220 px up to 16:1; png, jpeg; ≤ 10 references; BytePlus / Volcengine ids | `bytedance_seedream_5_0_pro_and_flash_are_carried_with_their_documented_contract`, `ark_model_id_sends_each_hosts_documented_id` | ✅ |
| `minimax/MiniMax-H3-Highspeed`, `-Lite` × route, `resolution`, `duration_seconds`, refs | the V2 API; 480P, 768P; int 4–15 s; first, last and ≤ 9 references | `the_new_h3_variants_go_to_the_v2_api`, `minimax_h3_highspeed_and_lite_are_carried_with_their_documented_contract` | 🟡 wiremock |
| `pixverse/v6-max` × model, `duration_seconds`, `resolution`, audio | `model: "v6-max"` on text/ and img/; int 1–15 s; 360p–1080p; `generate_audio_switch` | `v6_max_goes_to_text_and_img_by_its_own_id`, `pixverse_v6_max_is_carried_with_its_documented_contract` | 🟡 wiremock |
| `recraft/recraftv4_1_flash` × `size`, `seed`, `palette` | the V4.1 1K table; `random_seed`; `controls.colors` | `recraft_v4_1_flash_is_carried_with_its_documented_contract` | ✅ |
| `bfl/flux-3-image`, `-klein` × refs | one `images` list, in order, never `input_image_N`; `extra` cannot replace it | `flux_3_rows_send_every_reference_in_one_images_list` | 🟡 wiremock |
| `bfl/flux-3-image` × `output_format`, `seed`, `steps` | not declared, never sent (`additionalProperties: false`); `size` is declared (see below) | `output_format_goes_only_to_endpoints_that_take_it`, `bfl_flux_3_image_and_klein_are_carried_with_their_documented_contract` | 🟡 wiremock |
| `bfl/flux-3-image-klein` × `output_format`, `seed`, `steps`, `size` | png, jpeg; 0–4294967295; 1–16; width/height 64–6256 up to 16,822,272 px and 8:1 | the two tests above | 🟡 wiremock |
| FLUX 3 × `resolution` price | 0.75K / 1K / 1.5K / 2K / 4K (768sq … 4k) each at its tier | `varying_rows_price_the_variant_asked_for` | ✅ |
| BFL polls × a 503 | a 503 whose body names a status is the job's answer (FLUX 3: "A failed task can come back as HTTP `503` with a normal JSON body") | `a_failure_answered_with_a_503_ends_the_job` | 🟡 wiremock |
| image `resolution` (new canonical param) × Google, Kling image, BFL FLUX 3, Replicate FLUX 3 | one scale (`0.5K` … `4K`, `match_input`), default `1K`; Google `imageConfig.imageSize` (`0.5K` → `512`), Kling `resolution` (`1k`, `2k`), BFL / Replicate `resolution` (`0.75K` → `768sq`); the old extras (`imageSize`, `resolution`) are refused with "send it as `resolution`" | `an_image_resolution_is_one_of_the_models_tiers`, `every_declared_resolution_is_one_of_{googles_image_sizes,klings,bfls}`, `the_tier_goes_as_{klings,bfls}_resolution_and_extra_cannot_replace_it`, `flux_3_image_sends_its_references_as_images_and_its_tier_as_resolution`, `image_tiers_are_one_resolution_param_on_one_scale` | 🟡 wiremock |
| `layout` × BFL FLUX 3 Image, Klein | object and text boxes → `{id, bbox [top, left, bottom, right] 0–1000, desc}` rows appended to the prompt, each `<id>` named in the caption | `a_layout_goes_as_flux_3_boxes_in_the_prompt` | 🟡 wiremock |
| `size` × BFL FLUX 3 Image | width/height 256–6256 up to 16.8 MP and 8:1; over 4 MP with references refused before any call | `a_flux_3_image_edit_larger_than_4_mp_is_refused_before_any_call` | 🟡 wiremock |
| price ranges (`when: { megapixels: { gt, lte } }`) × FLUX 3 (pixel tiers), FLUX.2 pro / max / klein / flex (output MP bands), Seedream 5.0 Pro (≤ 2.61 MP) | the rule matches a number inside its bounds | `a_rule_can_name_a_range_of_a_numeric_fact`, `varying_rows_price_the_variant_asked_for` | ✅ |
| reference counts × Seedream 5.0 Pro, MiniMax H3 / Highspeed / Lite / Max | images past the vendor's free ones add their price | `varying_rows_price_the_variant_asked_for` | ✅ |
| `bfl/flux-2-flex` × `steps`, `guidance_scale`, refs | `steps`, `guidance`; up to 8 `input_image_N` | `bfl_flux_2_flex_is_carried_with_its_documented_contract` | ✅ |
| `vidu/viduq4-preview` × route, `audio`, `aspect_ratio` | img2video (first frame) or reference2video (1–15); `audio` on both; aspect ratio on reference2video only | `q4_preview_sends_its_audio_switch_on_both_endpoints`, `vidu_q4_preview_is_carried_with_its_documented_contract` | 🟡 wiremock |
| `replicate/flux-3-image` × refs, `resolution` | official versionless route; `images` list; tier as `resolution` | `flux_3_image_sends_its_references_as_images_and_its_tier_as_resolution`, `replicate_flux_3_image_is_carried_with_its_documented_contract` | 🟡 wiremock |
| Kling credentials | an access/secret key pair without an API key is refused naming the fix | `a_stored_key_pair_is_refused_naming_the_api_key_kling_needs` (image, video) | 🟡 wiremock |
| `ideogram/ideogram-4-5` × `size`, `quality`, `background`, `seed`, refs, mask, `region` | v2 multipart: `size` (32 presets), `quality`, `background`, `seed`; `images` with the edited one first, then references, then `mask`; a region drawn as the mask; `very_low` without an image, a size beside a mask and four others beside a mask refused | `ideogram_4_5_generates_with_its_own_fields`, `ideogram_4_5_sends_the_edited_image_first`, `what_ideogram_4_5_cannot_make_is_refused_before_any_call`, `ideogram_4_5_is_carried_with_its_documented_contract` | 🟡 wiremock |
| Vidu Q3, Q3 Mix, Q3 Pro Fast, Q2, Q2 Turbo, Q1 Classic × modes, `duration_seconds`, `resolution`, `audio` | each model's own operation: reference2video (Q3, Q3 Mix), img2video (Q3 Pro Fast), text2video (Q2), img2video + start-end (Q2 Turbo, Q1 Classic); `audio` only where the operation has it | `vidu_models_skipped_as_optional_are_carried_with_their_documented_contracts`, `the_newly_carried_models_send_audio_only_where_their_endpoint_has_it` | 🟡 wiremock |
| `vidu/viduq2-pro`, `viduq2-turbo` × `audio` price | "consumes an additional 15 credits" | `varying_rows_price_the_variant_asked_for` | ✅ |

## TokenHub (2026-11-04)

Tencent TokenHub, Hunyuan's new home, is the `tokenhub` provider (one Bearer
key); the old Tencent Cloud API stays as `hunyuan` for its existing buyers.
Facts in `catalog_conformance` (test first), wire shapes in
`providers/{image,video}/tokenhub.rs` wiremock tests. Ledger:
`.superpowers/sdd/2026-11-04-tokenhub-bedrock-fal/progress.md` (git-ignored).
None was run live.

| model × param | wire | proof | status |
|---|---|---|---|
| `tokenhub/hy-image-v3.5-preview` × prompt, refs | one `role: user` message: a `text` part, then each image as an `image_url` part (data URI) | `hy_image_3_5_sends_its_prompt_as_one_user_message_with_the_pooled_key`, `references_go_as_image_url_parts_after_the_text` | 🟡 wiremock |
| Hy-Image 3.5 × `resolution`, `aspect_ratio`, `size`, `seed` | `generate_max_pixels` 1048576 / 2359296 / 4194304; a ratio-shaped `size` beside it; an exact `size` alone (4K this way); `seed` | `the_tier_goes_as_generate_max_pixels_and_a_ratio_as_a_size_beside_it`, `every_declared_tier_and_ratio_maps_inside_the_vendors_bounds`, `a_size_with_a_tier_or_a_ratio_is_refused_before_any_call` | 🟡 wiremock |
| Hy-Image 3.5 × answer | `choices[0].delta.image.url` downloaded; a failure frame's `error` or a 422 is final | `a_failure_frame_or_a_refusal_names_the_vendors_reason` | 🟡 wiremock |
| `tokenhub/hy-video-v1.5` × prompt, `negative_prompt`, frame, `aspect_ratio`, `duration_seconds`, `resolution` | flat fields; `image` data URI; `aspect_ratio` only without an image; `duration` 5; `resolution` 720p | `text_to_video_sends_its_fields_with_the_pooled_key`, `image_to_video_sends_the_frame_as_a_data_uri_and_no_ratio` | 🟡 wiremock |
| Hy-Video × poll | `GET /wand/hunyuan-video/tasks/{task_id}`: queued, running, succeeded (`videos[0].url`), failed (`error`) | `every_documented_status_maps`, `every_video_and_3d_poll_fails_an_undocumented_state` | 🟡 wiremock |
| `tokenhub/wand-vega-video-1.0-lite` × prompt, frames, references | `content`: a `text` part, then `image_url` parts with `role` first_frame, last_frame or reference_image; frames and references not mixed | `wand_frames_and_references_go_as_roled_image_parts`, `wand_refuses_frames_with_references_and_a_last_frame_alone` | 🟡 wiremock |
| WAND-Vega × `resolution`, `aspect_ratio`, `duration_seconds` | `resolution` 768P (sent when none is asked) … 4K; `ratio`; int `duration` 4–15 | `wand_text_to_video_sends_a_content_list_and_its_settings`, `tokenhub_wand_vega_video_lite_is_carried_with_its_documented_contract` | 🟡 wiremock |
| WAND-Vega × poll | `GET /wand/vega-videos/tasks/{id}`: `content.video_url`, `error` | `wand_statuses_map` | 🟡 wiremock |

## Bedrock: Stability AI (2026-11-04)

| model × param | wire | proof | status |
|---|---|---|---|
| `bedrock/stability.*` × prompt, `negative_prompt`, `seed`, `output_format`, `aspect_ratio` | Stability's InvokeModel body, SigV4 for us-west-2; `aspect_ratio` on text-to-image only; no count | `sd3_5_large_sends_stabilitys_body_signed_for_us_west_2`, `bedrock_stability_rows_are_carried_with_their_documented_contract` | 🟡 wiremock |
| SD3.5 Large / Ultra × init image, `strength` | SD3.5 `mode: image-to-image` + `image` + `strength` (0.5 when none); Ultra `image` + `strength` when asked; Core refuses | `image_to_image_sends_the_image_and_strength_and_no_ratio` | 🟡 wiremock |
| Bedrock × answer | `images[0]`; a non-null `finish_reasons[0]` is a refusal | `a_filtered_answer_is_a_refusal_with_its_reason` | 🟡 wiremock |
| `tokenhub/kling-video-*` (v2.5-turbo, v2.6, v3, v3-turbo, v4) × prompt, frames, `duration_seconds`, `resolution`, `aspect_ratio`, `audio`, `multi_shot` | Kling's own `settings` / `contents` (shared with `video/kling.rs`), `model` in the body, frames as data URIs; TokenHub's rules (v2.5 frames 1080p; v2.6 frames 720p, audio 1080p) | `kling_text_to_video_sends_klings_shape_to_tokenhub`, `kling_image_to_video_sends_contents_with_data_uri_frames`, `tokenhubs_kling_rules_are_refused_before_any_call`, `tokenhub_kling_video_rows_are_carried_with_tokenhubs_contract` | 🟡 wiremock |
| TokenHub Kling × poll | `GET /wand/kling/tasks/{id}`: Kling's `data[]` (`kling_task_result`), a non-zero `code` an error | `kling_polls_map_tokenhubs_answers`, `every_video_and_3d_poll_fails_an_undocumented_state` | 🟡 wiremock |
| `tokenhub/seedream-image-v5.0-*` (pro, flash) × prompt, refs, `resolution`, `size`, `output_format`, `background` | Ark's body at `/wand/si-image/generation`: `size` a tier (1K / 1.5K / 2K) or WxH, never both; `images` data URIs; `response_format: b64_json` | `seedream_sends_arks_shape_with_a_tier_or_a_size`, `seedream_refuses_a_tier_with_a_size_and_keeps_its_owned_fields`, `tokenhub_seedream_rows_are_carried_with_tokenhubs_contract` | 🟡 wiremock |
| `tokenhub/minimax-video-v2.3`, `-v2.3-fast` × prompt, frames, `duration_seconds`, `resolution` | flat fields at `/wand/minimax-video/generation`: `first_frame_image` / `last_frame_image` data URIs, int duration (1080P only at 6 s, refused otherwise); `base_resp.status_code` ≠ 0 is a refusal | `minimax_flat_sends_its_fields`, `tokenhub_minimax_rows_are_carried_with_tokenhubs_contract` | 🟡 wiremock |
| `tokenhub/minimax-video-h3`, `-h3-max` × prompt, frames, references, `resolution`, `duration_seconds`, `aspect_ratio` | `content` at `/wand/minimax-video-v2/generation`: text, then image_url parts with role first_frame / last_frame / reference_image (not mixed); `ratio` on text-to-video (16:9 default); H3-Max `extra.prompt_expansion_mode` | `minimax_content_sends_roled_parts` | 🟡 wiremock |
| TokenHub MiniMax × poll | flat "Processing / Success / Fail", content task.status "queued … cancelled"; `content.url` | `minimax_polls_map`, `every_video_and_3d_poll_fails_an_undocumented_state` | 🟡 wiremock |
| `tokenhub/pixverse-video-*` (v6.0, c1, v5.6) × prompt, frames, references, `duration_seconds`, `resolution`, `aspect_ratio`, `seed`, `audio` | the endpoint from the images (text / `img_id` / `first_frame_img` + `last_frame_img` / `image_references[]`), `quality`, `generate_audio_switch`; `aspect_ratio` on text and references; v5.6 10 s at 1080p refused; `ErrCode` ≠ 0 a refusal | `pixverse_picks_its_endpoint_from_the_images`, `pixverse_refuses_what_it_cannot_make`, `tokenhub_pixverse_rows_are_carried_with_tokenhubs_contract` | 🟡 wiremock |
| TokenHub PixVerse × poll | `Resp.status` 1 / 5 / 6 / 7 / 8, `Resp.url` | `pixverse_polls_map`, `every_video_and_3d_poll_fails_an_undocumented_state` | 🟡 wiremock |
| `tokenhub/kling-image-*` (v3, o1, v3-omni) × prompt, refs, `negative_prompt` (v3), `aspect_ratio`, `resolution` | submit at `/wand/kling-image/generation` (`n: 1`; v3 one raw-base64 `image`, omni `image_list` + `result_type: single`; tiers 1k / 2k / 4k), the task recorded, polled at `/wand/kling-image/tasks/{id}`, `task_result.images[0].url` downloaded; still running at the deadline → `Timeout` (late delivery) | `kling_image_submits_polls_and_downloads`, `kling_omni_images_go_as_a_list_and_a_failure_names_its_reason`, `every_async_tier_is_one_of_the_vendors`, `every_resumable_image_job_is_read_by_poll_job` | 🟡 wiremock |
| `tokenhub/vidu-image-q2` × prompt, refs, `aspect_ratio`, `resolution`, `seed` | `/wand/vidu-image/generation` (`images` data URIs, 1K → "1080p"), polled at `/wand/vidu-image/tasks/{id}` (`state`, `creations[0].url`) | `vidu_image_submits_polls_and_downloads`, `every_image_poll_fails_an_undocumented_state` | 🟡 wiremock |
| `tokenhub/vidu-video-*` (q4-preview, q3-pro, q3-turbo, q3, q2, q2-pro, q2-turbo) × prompt, frames, references, `duration_seconds`, `resolution`, `aspect_ratio`, `audio`, `seed` | the endpoint from the images (as `video/vidu.rs`): `images` data URIs (start frame first), the row's 720p when none, `aspect_ratio` on text and references; refused: no-image Q4, sound on references but q3, a reference prompt over 2000, Q2 first/last over 8 s or with sound | `vidu_picks_its_endpoint_from_the_images`, `vidu_refuses_what_its_endpoint_cannot_take`, `vidu_q2_start_end_limits_are_refused_before_any_call`, `tokenhub_vidu_q3_and_q4_rows_are_carried_with_tokenhubs_contract`, `tokenhub_vidu_q2_rows_are_carried_with_tokenhubs_contract` | 🟡 wiremock |
| TokenHub Vidu × price | per second (Q3/Q4), first second + rate (Q2) as `per_second_usd` + `add_usd`, the reference endpoint's own figures by `references.reference` | `varying_rows_price_the_variant_asked_for` (7 cases) | ✅ |
| TokenHub Vidu × poll | `state` created / queueing / processing / success / failed, `creations[0].url` | `vidu_polls_map`, `every_video_and_3d_poll_fails_an_undocumented_state` | 🟡 wiremock |
| `tokenhub/yt-video-2.0` × first frame, prompt, `resolution`, `fps` | `/api/video/submit` with the old action's fields in snake_case (`image` {base64 \| url}); polled by POST `/api/video/query {model, id}` (`status`, `progress`, `data.url`) | `yt_video_submits_and_queries_by_post`, `tokenhub_yt_video_2_0_is_carried_with_tokenhubs_contract` | 🟡 wiremock |
| `tokenhub/kling-video-v3-omni`, `-o1` × prompt, frames, `audio` (v3-omni), `duration_seconds` | `/wand/kling/omni-video` for text and frames (`contents` + `settings`); O1 with a first frame alone only 5 / 10 s; no references (their count is undocumented for these two) | `kling_omni_sends_contents_to_its_own_endpoint` (pending a cargo slot), `tokenhub_kling_omni_rows_are_carried_with_text_and_frames` | 🟡 wiremock |
| `ideogram/nano-banana-2`, `-pro` × prompt, images, `aspect_ratio`, `resolution`, `seed` | Ideogram v2 `/v2/image/generate/{model}` multipart: `resolution_tier` (1K when none), the edited image first, `num_images` 1, no `async` / `webhook_url` | `nano_banana_through_ideogram_sends_its_own_fields` (pending a cargo slot), `ideogram_resold_nano_banana_rows_are_carried` | 🟡 wiremock |
| `replicate/sd3.5-large`, `-large-turbo`, `-medium` × prompt, `aspect_ratio`, `guidance_scale`, `seed`, `negative_prompt`, `strength`, `output_format`, init image | official versionless `/v1/models/stability-ai/stable-diffusion-3.5-*/predictions`; guidance as `cfg`, strength as `prompt_strength`, jpeg as jpg, no steps | `sd3_5_rows_send_cfg_to_their_own_official_model` (pending a cargo slot), `replicate_sd3_5_rows_are_carried_with_their_documented_contract` | 🟡 wiremock |
| `fal/flux-3-image`, `-klein` × prompt, images, `resolution` (Image), `aspect_ratio` (Image), `size` (Klein), `seed` (Klein), `output_format` | text-to-image at `blackforestlabs/flux-3[/klein]/text-to-image`; with images the `/edit-image` twin and an `image_urls` list; Image's tier as "1k" (only 1K priced), Klein's size as `image_size` | `flux_3_on_fal_routes_by_images`, `fal_flux_3_rows_are_carried_with_fals_contract` | 🟡 wiremock |
| `fal/flux-3-image-klein-base` × prompt, images, `size`, `seed`, `output_format` | Klein's body at `blackforestlabs/flux-3/klein/base/text-to-image/lora`, with images `…/klein/base/edit-image/lora` and `image_urls`; no `loras` sent; sizes up to 4,194,304 px (fal prices nothing past 2048²) | `flux_3_klein_base_on_fal_takes_its_lora_endpoints_with_no_lora`, `fal_flux_3_klein_base_is_carried_with_fals_contract` | 🟡 wiremock |
| `fal/flux-3-video`, `-video-draft`, `-max-video` × prompt, `first_frame`, `last_frame`, keyframes, `duration_seconds`, `resolution` (not drafts), `aspect_ratio`, `audio`, `seed` (Max) | no frame → `…/text-to-video`; a first frame → `…/image-to-video` (`image_url`); first + last → `…/first-last-frame-to-video` (`start_image_url`, `end_image_url`; Max: image-to-video's `end_image_url`); keyframes → `…/keyframes-to-video` (`frame_index` = round(time_s × 24)); duration an integer 5–20; on the standard row 1440p/2160p with frames or keyframes refused before the call; drafts at the `/draft` twins with no `resolution`; `audio` as `generate_audio` | `flux_3_video_on_fal_picks_its_endpoint_from_the_frames`, `flux_3_video_on_fal_refuses_what_its_endpoints_do_not_take`, `flux_3_max_video_on_fal_sends_its_last_frame_to_image_to_video`, `flux_3_video_drafts_on_fal_take_the_draft_endpoints`, `fal_flux_3_video_rows_are_carried_with_fals_contract` | 🟡 wiremock |
| `fal/flux-2`, `-turbo`, `-flash`, `-pro`, `-max`, `-flex`, `-klein-4b`, `-klein-9b`, `-klein-4b-base`, `-klein-9b-base` × prompt, images, `size`, `seed`, `output_format`, `guidance_scale` (dev, turbo, flash, flex, klein base), `steps` (dev, flex, klein), `negative_prompt` (klein base) | text-to-image at each endpoint; with images its `/edit` twin and an `image_urls` list (4 images on dev, turbo, flash, klein; 8 on pro, max, flex); size as `image_size` {width, height}; `steps` as `num_inference_steps`; no `num_images` | `flux_2_on_fal_routes_by_images`, `fal_flux_2_rows_are_carried_with_fals_contract`, `fal_flux_2_matches_the_endpoint_input_schema` | 🟡 wiremock |
| `fal/flux-2-pro-outpaint` × `expand`, the init image, `output_format` | `fal-ai/flux-2-pro/outpaint`: `image_url`, `expand_left` / `_top` / `_right` / `_bottom` (≤ 2048), no prompt; `extra` adds `mode`, `auto_crop` and can't change a side; nothing to grow is refused before the call | `flux_2_pro_outpaint_on_fal_sends_the_sides_and_no_prompt`, `fal_flux_2_pro_outpaint_is_carried_with_fals_contract` | 🟡 wiremock |
| `fal/flux-kontext-pro`, `-max`, `fal/flux-pro-1.1-ultra`, `-1.1-redux`, `-1.1-ultra-redux`, `fal/flux-pro-fill`, `-erase` × prompt, images, `aspect_ratio` (Kontext, ultra), `guidance_scale`, `steps`, `size` (Redux), mask and `region` (fill, erase) | Kontext: no image → `…/kontext[/max]/text-to-image`, one → `…/kontext[/max]` (`image_url`), two to four → `…/multi` (`image_urls`); ultra's `redux` image prompt as `image_url`; fill and erase send the mask as `mask_url` and refuse a request without one; erase sends no `prompt` | `flux_1_pro_on_fal_routes_by_images_and_masks`, `fal_flux_1_pro_rows_are_carried_with_fals_contract` | 🟡 wiremock |
| `fal/recraft-v2`, `-v4`, `-v4-pro`, `-v4-vector`, `-v4-pro-vector`, `-v4.1`, `-v4.1-pro`, `-v4.1-vector`, `-v4.1-pro-vector`, `-v4.1-utility`, `-v4.1-utility-pro`, `-v4.1-flash` × prompt, `size`, `palette`, `style` (V2) | each row's own endpoint (V2 at the listed `fal-ai/recraft-20b`); `size` from Recraft's 1K or 2K table as `image_size`; `palette` as `colors` [{r, g, b}]; SVG rows answer fal's `image/svg+xml` | `recraft_v4_on_fal_sends_its_size_and_palette`, `fal_recraft_v4_rows_are_carried_with_fals_contract` | 🟡 wiremock |
| `fal/recraft-v3` (images), `-v4-style`, `-v4-style-pro`, `-v4-style-vector`, `-v4-style-pro-vector`, `-vectorize`, `-upscale-crisp`, `-upscale-creative` × the init image, `strength` and `negative_prompt` (V3 images), `style` references, `palette` | V3 with an image → `/image-to-image` (`image_url`, `strength`, `negative_prompt`; text-to-image gets neither); style rows send `style` references as `image_urls` (+$0.005 when the style is made from them); vectorize and upscale send `image_url` alone and read fal's one `image` | `recraft_operations_on_fal_send_their_own_fields`, `fal_recraft_operations_are_carried_with_fals_contract` | 🟡 wiremock |
| `ideogram/p-image-ideogram` × prompt, `quality`, `resolution`, `aspect_ratio`, `seed` | `POST /v2/image/generate/p-image-ideogram` as JSON: `quality` (medium when none is asked), `resolution` "1k" \| "2k", `aspect_ratio` "16x9", `seed`, `num_images` 1 | `p_image_through_ideogram_sends_its_json_fields`, `ideogram_p_image_is_carried_with_ideograms_contract` | 🟡 wiremock |

### 2026-11-09 audit fixes

From `docs/superpowers/2026-11-09-provider-audit.md`, each vendor fact re-read
that day.

| param × provider | what reaches the vendor (or what is billed) | proof | status |
|---|---|---|---|
| ref role `reference` × `vidu/viduq3-turbo` | not offered; the adapter refuses one before any call (Vidu documents only the subjects form for viduq3-turbo on reference2video) | `vidu_q3_turbo_serves_every_endpoint_the_adapter_routes_to`, `viduq3_turbo_refuses_reference_images_before_any_call` | 🟡 wiremock |
| `extra.movement_amplitude`, `extra.bgm` × `vidu/viduq1` | sent on start-end2video and reference2video, left out on text2video (its body has neither) | `viduq1_text2video_sends_no_movement_amplitude_or_bgm` | 🟡 wiremock |
| `aspect_ratio` × `minimax/MiniMax-H3-Lite` reference-to-video | the row's default (16:9) when none is asked: H3-Lite "does not support `adaptive` yet" | `h3_lite_references_never_ride_on_the_adaptive_default` | 🟡 wiremock |
| `aspect_ratio` × `tokenhub/kling-image-o1`, `-v3-omni` text-to-image | 16:9 when none is asked (TokenHub's own `auto` is refused for text-to-image); with images, none, so `auto` follows them | `kling_omni_text_to_image_never_rides_on_the_auto_default` | 🟡 wiremock |
| result `content_type` × Replicate, BFL | the bytes' type (the router already relabelled by bytes) | `a_result_is_labelled_with_the_type_its_bytes_are` (both adapters) | 🟡 wiremock |
| price × `openai/gpt-image-2` `quality` | low $0.006, medium $0.053, high $0.211 at 1024² | `varying_rows_price_the_variant_asked_for`, `an_openai_image_is_quoted_by_its_quality_and_its_size`, `gpt_image_2_estimate_is_openais_table_by_quality` | 🟡 |
| price × Ideogram 3.0 `character` reference | $0.15 / $0.10 / $0.20 (Default / Turbo / Quality) | `varying_rows_price_the_variant_asked_for` | 🟡 |
| price × `luma/ray-2`, `luma/dream-machine` `resolution` | 1080p $0.95, 4k $1.05 (Luma's 5 s figures) | `varying_rows_price_the_variant_asked_for` | 🟡 |
