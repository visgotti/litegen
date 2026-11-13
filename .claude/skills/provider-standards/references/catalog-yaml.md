# Catalog YAML (`models/<provider>.yaml`)

The catalog is the contract. `capabilities/loader.rs` loads it at boot (bad
YAML keeps the server from starting). `GET /v1/models` serves it,
`api/middleware/validator.rs` enforces it, and the Playground, the dashboard's
Models page, the landing site and SDK users all render from it. No other code
holds a model list.

## Contents

- [A row, annotated](#a-row-annotated)
- [Ids, names and descriptions](#ids-names-and-descriptions)
- [Params](#params)
- [Reference inputs](#reference-inputs)
- [Capabilities, tags and required modes](#capabilities-tags-and-required-modes)
- [extra_allowlist and strict mode](#extra_allowlist-and-strict-mode)
- [Pricing](#pricing)
- [Docs and evidence](#docs-and-evidence)
- [Rules the schema can't express](#rules-the-schema-cant-express)

## A row, annotated

```yaml
  # Acme Photo 2: the same POST /v2/images endpoint as Photo 1, sent as model `photo-2`.
  # @see https://docs.acme.example/openapi.json (2026-10-12) — CreateImageRequest
  - id: acme/photo-2                 # "<provider>/<name>"; prefix must equal `provider`
    provider: acme                   # the registry key, shared by every family
    media_type: image                # image | video | model3d
    display_name: Acme Photo 2
    description: Acme Photo 2 text-to-image and image-to-image with style references.
    docs_url: https://docs.acme.example/reference/create-image   # human API page for THIS endpoint
    # "$0.04 per image (standard), $0.08 per image (hd)"; the default request
    # is standard → $0.04.
    # @see https://acme.example/pricing.md (2026-10-12)
    pricing:
      base_cost_usd: 0.04
      variable_pricing:
        - { when: { quality: hd }, usd: 0.08 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 2000 }   # the vendor's documented limit
    params:
      # CreateImageRequest.aspect_ratio enum (the line above cites the schema).
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9", "9:16", "4:3", "3:4"]
        default: "1:1"
      seed: { kind: seed, min: 0, max: 2147483647 }
      negative_prompt: { kind: string, max_length: 1000 }
    ref_inputs:
      max_total: 4
      default_role: init
      provider_format: { form: base64 }
      roles:
        init:  { required: false, min_count: 0, max_count: 1 }
        style: { required: false, min_count: 0, max_count: 3 }
    extra_allowlist: [safety_tolerance]   # vendor-only fields, and only ones THIS endpoint takes
    extra_params:                         # each allowlisted key typed as the schema states it
      safety_tolerance: { kind: int, min: 0, max: 6 }   # CreateImageRequest.safety_tolerance
    tags: [text-to-image, image-to-image]
```

The loader (`validate_model` in `capabilities/loader.rs`) rejects the
following:

- an id that isn't `provider/name`, or a prefix that doesn't match `provider`;
- a negative price;
- param keys outside `KNOWN_PARAMS`, or a param declared with a kind other
  than its canonical kind;
- a `docs_url` that isn't an absolute `https://` URL;
- `prompt.required: true` together with `max_length: 0`;
- `prompt.required_for_text_to_video: true` on a row that is not a video
  model with `text_to_video`;
- `needs_first_frame: true` on a role other than a video model's
  `last_frame`, or on a row with no `first_frame` (or `init`) role;
- a freeform size with min > max;
- a multipart `field_map` or `default_role` naming an undeclared role, and
  a `provider_format` with a key its `form` doesn't take
  (`{ form: url, field_map: … }`);
- `preserves_unmasked` on a row that is not an image model taking a `mask`
  role or a `region` param;
- an `extra_allowlist` entry litegen owns (see
  [extra_allowlist](#extra_allowlist-and-strict-mode)), and `imageId` or
  `imageType` on a Leonardo video row (its start frame is the request's own
  image, which litegen uploads);
- an `extra_params` entry that isn't allowlisted, isn't a plain kind, is one
  of the row's params, or has a default it doesn't take, and a price rule
  that reads an untyped `extra.<key>`;
- a `pattern` that isn't a regular expression;
- a `litegen_params` key (the server computes it, like
  `output_formats.derivable_values`);
- 3D `output_formats` problems, covered in the model3d-provider skill.

The registry (`capabilities/registry.rs`) also rejects duplicate ids across
files.

A key the schema doesn't know is refused too, at every level of a row
(`deny_unknown_fields` on each catalog struct): a typo'd key in
`models/*.yaml` keeps the server from starting, and a runtime model posted
with one gets 400, naming the field. The one exception is `status`, which is
read and ignored, since litegen derives it from the dates. Keep to the
documented fields.

**Layout matters to tooling, not only to serde.** Each file is a top-level
`models:` list. The drift tooling (`loadRegistry` in
`scripts/model-drift/lib.mjs`) parses the YAML line by line. Start each row
with its own `- id:` line and give it exactly one `media_type:` line, as every
existing file does. Don't write rows in flow style (`{ id: …, … }`) or with
anchors.

## Ids, names and descriptions

- When the vendor's model id is URL-safe and stable, use it as `<name>` so
  the adapter can pass it through verbatim (`bfl/flux-2-pro`, `vidu/viduq1`).
  Otherwise pick a stable slug and map it explicitly in the adapter.
- Ids are unique across every file and every family. When a vendor reuses a
  model name across families, prefix one of them (Kling's video rows are
  `kling/video-…`).
- Never reuse a retired id for a different model. The retired list in
  `catalog_conformance.rs` guards against it.
- For aggregator-hosted models, `display_name` names the host:
  "TRELLIS (Fal)", "Hyper3D Rodin (Fal)".
- `description` is one sentence: what the model does and its modes. Start it
  with `DEPRECATED: <date> …` once the vendor announces a shutdown.

## Params

Declare only params from your family's request struct. A param outside it
never reaches the adapter, even if the YAML declares it.

| Family | Params the request carries |
|---|---|
| all | `seed`, `negative_prompt` (on `BaseGenerationRequest`) |
| image | `size`, `aspect_ratio`, `resolution`, `quality`, `style`, `steps`, `guidance_scale`, `strength` |
| video | `duration_seconds`, `aspect_ratio`, `resolution`, `fps` |
| model3d | `output_formats`, `texture`, `pbr`, `target_polycount`, `topology`, `symmetry`, `rig` |

The canonical kind for each param (`canonical_param_kinds` in
`capabilities/schema.rs`):

| Param | `kind` | Notes |
|---|---|---|
| `seed` | `seed` | `min` and `max` are required. Use the vendor's real range (Rodin is u16, most are u32). |
| `negative_prompt`, `quality`, `style`, `resolution` | `string` | `enum_values` holds the vendor's exact spellings; `max_length` applies to free text. An image `resolution` is the exception: it is on litegen's one scale (`0.5K`, `0.75K`, `1K`, `1.5K`, `2K`, `4K`, plus FLUX 3 Image's `match_input`), default `1K`, and the adapter maps it to its vendor's spelling through a named table (`image_size` in google.rs, `kling_resolution`, `bfl_resolution`, `replicate_resolution`). A vendor's tier never goes in `extra_allowlist` (`image_tiers_are_one_resolution_param_on_one_scale`). |
| `steps`, `fps` | `int` | `min`, `max`, `default` |
| `guidance_scale`, `strength` | `float` | `min`, `max`, `default` |
| `duration_seconds` | `float` or `int` | Most vendors take fractional seconds; Runway takes whole ones. |
| `size` | `size` | `mode: enum` with `values: [[w,h],…]` (these feed `supported_sizes`), or `mode: freeform` with `min_width`/`max_width`/`min_height`/`max_height`/`multiple_of` |
| `aspect_ratio` | `aspect_ratio` | `allowed` holds the vendor's exact strings, plus an optional `default`. A video row whose frame image sets the clip's shape adds `frames_set_shape: always \| without_references` (see "One list serves every mode") |
| `output_formats` | `string_array` | 3D only; see model3d-provider |
| `output_format`, `symmetry`, `topology` | `string` | 3D; `output_format` is the superseded scalar |
| `texture`, `pbr`, `rig` | `bool` | 3D |
| `target_polycount` | `int` | 3D |

Every spec takes an optional `label` and `description`, which the Playground
and aipix render as the control's label and help text. Add them when the
vendor's meaning differs from the obvious one ("Off sends Rodin's
`material=Shaded`"). In a flow mapping (`{ kind: bool, description: … }`),
quote a description that holds a comma: YAML ends a plain scalar at `, `,
so the text would be cut there and the rest read as a key, which the loader
refuses. `every_param_description_is_a_whole_sentence` checks that each
description ends a sentence.

How validation treats a param:

- If a model declares the param, the value is checked against its spec
  (range, enum or length) in both strict and lax mode. An out-of-range value
  is always a 4xx.
- If a model doesn't declare it, strict mode returns `param_unsupported` and
  lax mode drops it and lists it in `X-Litegen-Dropped-Params`. This is how a
  client can send a superset of params without its own capability table.
- `duration_seconds` follows the same rule: on a row that doesn't declare
  it, it is refused when strict and dropped and named otherwise (its clip
  length is the vendor's, or its keyframes'). On a row that does, the
  adapter receives the caller's value, or else the row's declared `default`,
  or else 5.0 (`resolve_duration_seconds`), and a value of 0 or less is
  refused in both modes. A model whose minimum is above 5 s must declare a
  `default`.
- A `string` spec's `pattern` is a regular expression the value must match
  anywhere in it, as JSON Schema's `pattern` does (anchor it with `^…$`). A
  value it doesn't match is refused (`param_invalid`), and the loader
  refuses a pattern that doesn't compile.
- `prompt` takes `required` (default **true**), `min_length` and
  `max_length`. Prompt lengths count characters, like JSON Schema's
  `maxLength`, so use the vendor's limit as written. String params
  (`negative_prompt`, `quality`, `style`, `resolution`) count `max_length`
  in characters too.
- `prompt` also takes `required_for_text_to_video`. On a video row whose
  vendor needs the prompt for text-to-video but not for image-to-video,
  set `required: false, required_for_text_to_video: true`: a request with
  no `first_frame` or `init` image and an empty prompt is then refused with
  `prompt_required` before anything is quoted or reserved
  (`check_video_request`). Leave it off where the vendor's text-to-video
  runs without a prompt: Runway's Kling 3.0 and O3 branches don't require
  `promptText`, and their shots can carry the prompts in
  `providerSettings.multiPrompt`.
- A model whose endpoint has no prompt field at all (TRELLIS, for example)
  declares `prompt: { required: false, max_length: 0 }`. Strict callers then
  get `param_unsupported` and lax callers have the prompt dropped, so it
  neither vanishes silently nor reaches a schema that rejects it. Read
  `check_prompt` in `validator.rs` for the exact rules.

**One list serves every mode.** A row has one `aspect_ratio` list, one
`resolution` list and one duration range, whatever mode the request uses.
When the vendor's text-to-X and image-to-X endpoints accept different sets,
the row must offer only values valid for every mode it advertises (MiniMax
and Runway have conformance tests for exactly this). If the difference
matters, intersect the sets or split the model into two rows.

A video endpoint that takes no ratio because the frame image sets the
clip's shape is a per-mode rule the schema does express. Declare
`frames_set_shape` on the row's `aspect_ratio`: `without_references` when a
`reference` image beside the frame reaches a mode that takes a ratio (or the
vendor refuses that mix), `always` when it changes nothing. With a
`first_frame`, `last_frame` or `init` image, the validator then refuses the
ratio when strict and drops and names it otherwise.
`every_row_whose_frames_set_the_shape_declares_it` in
`catalog_conformance.rs` lists every row that declares it, with its value.

## Reference inputs

```yaml
ref_inputs:
  max_total: <n>                 # across all roles
  default_role: <role>           # applied to refs sent without a role
  provider_format: { form: url | base64 }
  # or, for a multipart endpoint:
  # provider_format: { form: multipart, field_map: { init: image, mask: mask } }
  roles:
    <role>: { required: <bool>, min_count: <n>, max_count: <n> }
    # a video row whose vendor ends a clip on a last frame only after a first:
    # last_frame: { required: false, min_count: 0, max_count: 1, needs_first_frame: true }
```

**Role names carry meaning, so reuse the existing vocabulary:**

| Family | Roles in use | Meaning |
|---|---|---|
| image | `init` | the image-to-image source |
| image | `mask` | inpainting |
| image | `style`, `character`, `subject`, `modify` | Luma, Kling and Ideogram reference kinds |
| image | `redux` | BFL image prompt |
| video | `first_frame`, `last_frame` | keyframes; **these exact names** set `supports_first_frame` / `supports_last_frame` in `GET /v1/models` |
| video | `reference` | reference-to-video |
| video | `subject` | subject reference |
| model3d | `init` | image-to-3D |
| model3d | `view-front`, `view-back`, `view-left`, `view-right`, `view-top` | multiview (at least 2) |

Some older video rows use `init` for the start frame (`fal/video`,
`fal/ltx-2.3`, `runway/gen4-turbo` and `runway/gen4.5`), so their
`supports_first_frame` reads false. New video rows use `first_frame`.

**A last frame that needs a first.** When the vendor ends a clip on a last
frame only after a first one, so the adapter refuses a `last_frame` sent
alone, set `needs_first_frame: true` on the row's `last_frame` role. A
request with a `last_frame` and no `first_frame` (or `init`) image is then
refused with `ref_role_required` before anything is quoted or reserved
(`check_video_request`), ahead of the prompt check, since a prompt wouldn't
help. Keep the adapter's refusal as a backstop. Today that is every row of
BytePlus, fal's FLUX 3, Ideogram, Kling, PixVerse, Runway's resold models,
TokenHub and Vidu. Leave it off where the vendor takes a last frame alone:
Luma's `frame1`, Ray 2 on Bedrock, MiniMax's first-last-frame endpoint
(`required: model, last_frame_image`). `tests/pricing_api.rs` lists each
vendor's answer with its source and fails on a vendor it doesn't know.

**How refs are materialized** (`proxy/materializer.rs`). Callers send each ref
as a `url`, `base64` or a multipart `blob`, and the materializer converts it to
the row's `provider_format`:

| Caller sends | → `url` | → `base64` | → `multipart` |
|---|---|---|---|
| url | passed through | fetched server-side (SSRF-guarded, 20 MiB cap) and encoded | fetched, as `application/octet-stream` |
| base64 | **not a fetchable URL**: an unusable `local://…` URL at HEAD, or an inline `data:` URI after an in-progress change (see known-gaps.md) | passed through (data: prefix stripped) | decoded |
| blob | same as base64 | encoded | passed through |

The materializer is changing as this is written (MIME sniffing, `data:` URIs).
Read `Materializer::convert` for the exact behavior in your checkout,
including the content type a multipart part carries. If the vendor checks
part types, set them from the bytes with `sniff_image_mime`.

Choose the form like this:

- **`base64`** when the vendor accepts it. It works for every way a caller can
  send an image.
- **`url`** only when the vendor takes nothing else. Such models only work
  with caller-supplied public URLs, unless the vendor also accepts `data:`
  URIs in that field (verify this live). Alternatively, declare
  `form: multipart` so the adapter receives raw bytes, and upload them to
  the vendor's own file API inside the adapter, as Leonardo's
  `upload_init_image` and PixVerse's image upload do. An upload URL taken
  from the vendor's answer (a presigned URL) is untrusted: with the
  adapter's `public_assets_only` on, pass it to
  `util::ssrf::check_public_url` before sending anything there, and send
  the upload on a public-only client that follows no redirect
  (`util::ssrf::public_only_client_with_timeout`, or the adapter's
  `public_only_download_client` asset client), never the vendor API
  client. Leonardo's `upload_init_image` takes a `public_only` argument for
  this (SEC-LEONARDO).
- **`multipart`** when the vendor endpoint is `multipart/form-data` (Ideogram,
  Stability, OpenAI edits). `field_map` maps each role to its form field.

Validation applies roles as follows: a ref without a role gets
`default_role`; an unknown role is rejected in strict mode and dropped in
lax mode; counts are checked per role and against `max_total`; a model with
no `ref_inputs` rejects every ref in strict mode. Refs reach the adapter as
`materialized.refs` (`role`, `form`), and **the adapter decides the mode from
the roles present**.

## Capabilities, tags and required modes

- Set `capabilities` to the modes the adapter actually implements for this
  row, verified against the vendor's capability map:
  - image: `text_to_image`, `image_to_image`, `inpainting`
  - video: `text_to_video`, `image_to_video`
  - 3D: `text_to_3d`, `image_to_3d`, `multiview_to_3d`
- These flags feed `GET /v1/models` (and the landing site), and the
  validator enforces them: a request whose refs ask for a mode the row doesn't
  advertise is refused with 400 `capability_unsupported` before any vendor
  call (`check_image_request`, `check_video_request`, and 3D's text-only
  check in `api/middleware/validator.rs`). A model that cannot run without an
  image still declares its role `required: true, min_count: 1`, so the
  catalog shows it. The adapter must also refuse a request
  whose roles imply an unsupported mode, with `ProviderError::InvalidRequest`,
  before any vendor call (vidu's endpoint selection, fal 3D's empty arms).
  The caller gets 400 `validation_error` with code `request_unsupported`,
  and nothing is billed (SKILL.md rule 13).
- `tags` are display labels. Reuse the existing vocabulary: `text-to-image`,
  `image-to-image`, `inpainting`, `text-to-video`, `image-to-video`,
  `reference-to-video`, `fast`, `premium`, `preview`, `vector`, `3d`, `mesh`.
  Add a family tag (`rodin`, `trellis`) where it helps.

## extra_allowlist and strict mode

`extra` is the escape hatch for vendor fields with no canonical param. List in
`extra_allowlist` only fields that **this row's endpoint** accepts, verified in
its schema. A field valid on a sibling model doesn't count: PixVerse's audio
switch exists only from v5.5, and Vidu Q3 takes `audio` but not `bgm` ("BGM
does not available in q3 models"). Conformance tests pin several of these
lists.

The allowlist holds in either mode: with `strict: true` (the default) an
unlisted key is refused (`extra_key_unsupported`), and with `strict: false`
it is dropped and reported (`X-Litegen-Dropped-Params: extra.<key>`). Only
allowlisted keys reach the adapter, so never allowlist a field the adapter
owns (rule 11 in SKILL.md).

Never allowlist a field litegen owns either: where the vendor sends a
result (a callback or webhook URL), where it stores one, or whether it
publishes one. litegen polls every job itself and fires the key's own
webhook. `capabilities::gateway_owned_extra` names them: `callback_url`,
`webhook_url`, `notify_url`, `storage_uri`, `output_data_config`,
`is_public`, `public`, and any key containing `callback` or `webhook`,
compared without case, `_` or `-`. The loader refuses such an entry, a
request's `extra.<key>` naming one is refused when strict (with the reason)
and dropped otherwise, and `providers::extra_fields`, the filter every
adapter merges `extra` through, never sends one.

Type every allowlisted key in `extra_params`, by key, with the vendor's own
JSON type: `bool`, `int`, `float`, `string` or `string_array`, with the
bounds, values or `pattern` the vendor states (the kind alone where it
states none), and a comment saying where the type comes from:

```yaml
    extra_allowlist: [prompt_upsampling, safety_tolerance]
    extra_params:
      # From BFL's OpenAPI per endpoint (drift snapshot bfl/openapi.txt).
      prompt_upsampling: { kind: bool, default: false }
      safety_tolerance: { kind: int, min: 0, max: 6, default: 2 }
```

A request's value is then checked like a param's, in either mode
(`param: "extra.<key>"`); `null` passes as sent. A key whose values are JSON
objects or integer lists, which no kind describes, or with no evidence of its
type, stays untyped: name it in `UNTYPED` in
`every_allowlisted_extra_is_typed_or_named_untyped` (`catalog_conformance.rs`)
with the reason. That test also pins how many entries are typed and untyped,
so update its counts. A price rule may read only a typed `extra.<key>`: it
compares the value exactly, and `true` is not `"true"`.

A request field its request type doesn't have, at the top level or inside
one of its objects (`reference_images[0].weight`), follows the same rule:
`param_unsupported` when strict, dropped and named in
`X-Litegen-Dropped-Params` otherwise. A top-level field the row allowlists
is moved into `extra`, as if sent there, in either mode, and judged like any
`extra` key (the OpenAI SDK sends `user` and `moderation` at the top level).
Sent both ways with different values, it is refused when strict
(`param_invalid`), and `extra`'s value is kept otherwise.

If users need a knob that the UI should render, or that more than one vendor
shares, it should become a canonical param. That is a core change, so ask the
user first.

## Pricing

- `base_cost_usd` is the vendor's price for the row's default request, and
  `variable_pricing` prices every other variant. Both are applied
  (`capabilities::pricing`): the quote, the quota reservation and the charge
  all come from them, plus `cost_markup_percent`, times the images delivered
  for images. Video, 3D and late images are charged when the vendor accepts
  the job, and refunded if it fails. An adapter's own `estimate_cost` result
  is discarded.
- Put the vendor's price string in a comment, quoted, with the arithmetic and
  `@see <pricing url> (YYYY-MM-DD)`:
  `# Default request (720p, 5 s): $0.055/s × 5 s = $0.275.`
- Price every variant the vendor prices differently, as `variable_pricing`
  rules applied in order to a running price that starts at
  `base_cost_usd`. A rule's `when` names request facts, and its one effect
  sets a price (`usd`), a rate (`per_second_usd`, `per_megapixel_usd`,
  `per_item_usd` with `per_item`), or adjusts it (`multiply`, `add_usd`,
  `multiply_per_megapixel_above`). The facts are a param (as sent, else its
  default), an allowlisted `extra.<field>` (typed in `extra_params`), or a
  derived fact:
  `duration_seconds`, `megapixels`, the `views`, `keyframes` and
  `animations` counts, `added_views` (the views besides `view-front`: a
  vendor's multi-view add-on), `references` (every reference sent) and
  `references.<role>`. A fact may also be a range a number falls in,
  `{ gt | gte | lt | lte: n }`, alone or together: a pixel tier is
  `{ when: { size: '*', megapixels: { gt: 1.048576, lte: 2.359296 } }, usd: … }`
  (`megapixels` is the size's pixels ÷ 1,000,000, and 1 MP when no size is
  sent, hence `size: '*'`), a count past a free quota is an exact
  `references: 6` with `add_usd`. A "first second plus a rate after" price is a
  `per_second_usd` rate plus an `add_usd` fixed part (Vidu Q2 Pro).
  `base_cost_usd` must equal what the rules charge the default request
  (`every_base_price_is_what_the_rules_charge_for_the_default_request`).
- The landing reference page words every rule
  (`apps/landing/scripts/derive-models.mjs`), and a rule it can't word stops
  `sync-models`, and with it the landing build. Four shapes stop it: an
  effect outside its `PRICE_EFFECTS` (the ones `pricing.rs` applies); a
  `"*"` on a fact every request has
  (`duration_seconds`, `megapixels`, or a param with a default); an exact
  value equal to a param's default that `param_default` doesn't apply
  (`aspect_ratio`, `size`); and an image row with a `directions` param. Fix
  the rule, or teach the derivation the new shape.
- A price the rules can't express (by a step count, by a range only the
  vendor's calculator knows) goes in the comment and the report; the row
  bills the default request's price for it.
- Vendors that price in credits or units: show the conversion
  (`80 credits ($0.40)`). Prices only behind a login: say so and mark the
  price unverified (the Leonardo rows do this).

## Docs and evidence

Each row carries three kinds of evidence:

1. **`docs_url`**: the human-readable API reference for the exact endpoint
   this row calls. For fal that is `https://fal.ai/models/{endpoint_id}/api`.
   It is served on `GET /v1/models/{id}`, the loader requires an absolute
   https URL, and `tests/model3d_conformance.rs` requires it on every vendor
   3D row. Set it on image and video rows too.
2. **Typings evidence**: `# @see <machine-readable schema url> (YYYY-MM-DD)`
   above the params it proves, naming the schema type
   (`CreateImageRequest.aspect_ratio`). Quote the vendor's exact words for
   rules that aren't obvious ("In the Image-to-Image scenario … negative
   prompts are not supported.").
3. **Pricing evidence**: the quoted price line and `@see <pricing url> (date)`
   beside `pricing:`.

Keep the date on every `@see`. Evidence without a date can't tell the weekly
drift run, or a reviewer, how stale it is.

## Rules the schema can't express

Some vendor rules have no YAML representation today. List each one in your
report under "Can't express", along with what you advertised instead:

- **Discrete numeric sets**, such as "5 or 10 seconds only" or "4, 6 or
  8 s". The schema only knows min and max. Advertise the range, and have the
  adapter snap deterministically to an allowed value (vidu's
  `resolve_duration`) or reject with `InvalidRequest` before submitting
  (400 `request_unsupported`, nothing billed). Test that behavior.
- **Per-mode value sets.** Intersect them, or split the model into two rows
  (see "One list serves every mode" above).
- **Coupled params**, such as "1080p only at 5 s" or "negative_prompt only in
  text-to-image". Enforce them in the adapter with `InvalidRequest` before
  any vendor call, or drop the field in the mode that forbids it, and
  document it in a YAML comment.
