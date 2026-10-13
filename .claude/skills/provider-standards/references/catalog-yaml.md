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
    # "$0.04 per image (1024x1024)"; default request is 1024x1024 → $0.04.
    # 2048x2048 is $0.08 (×2), billed flat at the base here.
    # @see https://acme.example/pricing.md (2026-10-12)
    pricing:
      base_cost_usd: 0.04
      variable_pricing: { size_2048_multiplier: 2 }   # documentation only; never applied
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
    extra_allowlist: [output_format]   # vendor-only fields, and only ones THIS endpoint takes
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
- a freeform size with min > max;
- a multipart `field_map` or `default_role` naming an undeclared role;
- 3D `output_formats` problems, covered in the model3d-provider skill.

The registry (`capabilities/registry.rs`) also rejects duplicate ids across
files.

Unknown top-level keys are ignored silently, so a typo'd key costs you the
feature with no error. Keep to the documented fields.

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
| image | `size`, `aspect_ratio`, `quality`, `style`, `steps`, `guidance_scale`, `strength` |
| video | `duration_seconds`, `aspect_ratio`, `resolution`, `fps` |
| model3d | `output_formats`, `texture`, `pbr`, `target_polycount`, `topology`, `symmetry`, `rig` |

The canonical kind for each param (`canonical_param_kinds` in
`capabilities/schema.rs`):

| Param | `kind` | Notes |
|---|---|---|
| `seed` | `seed` | `min` and `max` are required. Use the vendor's real range (Rodin is u16, most are u32). |
| `negative_prompt`, `quality`, `style`, `resolution` | `string` | `enum_values` holds the vendor's exact spellings; `max_length` applies to free text. |
| `steps`, `fps` | `int` | `min`, `max`, `default` |
| `guidance_scale`, `strength` | `float` | `min`, `max`, `default` |
| `duration_seconds` | `float` or `int` | Most vendors take fractional seconds; Runway takes whole ones. |
| `size` | `size` | `mode: enum` with `values: [[w,h],…]` (these feed `supported_sizes`), or `mode: freeform` with `min_width`/`max_width`/`min_height`/`max_height`/`multiple_of` |
| `aspect_ratio` | `aspect_ratio` | `allowed` holds the vendor's exact strings, plus an optional `default` |
| `output_formats` | `string_array` | 3D only; see model3d-provider |
| `output_format`, `symmetry`, `topology` | `string` | 3D; `output_format` is the superseded scalar |
| `texture`, `pbr`, `rig` | `bool` | 3D |
| `target_polycount` | `int` | 3D |

Every spec takes an optional `label` and `description`, which the Playground
and aipix render as the control's label and help text. Add them when the
vendor's meaning differs from the obvious one ("Off sends Rodin's
`material=Shaded`").

How validation treats a param:

- If a model declares the param, the value is checked against its spec
  (range, enum or length) in both strict and lax mode. An out-of-range value
  is always a 4xx.
- If a model doesn't declare it, strict mode returns `param_unsupported` and
  lax mode drops it and lists it in `X-Litegen-Dropped-Params`. This is how a
  client can send a superset of params without its own capability table.
- `duration_seconds` is the exception. An undeclared duration is accepted,
  and the adapter receives the caller's value, or else the row's declared
  `default`, or else 5.0 (`resolve_duration_seconds`). A model whose minimum
  is above 5 s must declare a `default`.
- `prompt` takes `required` (default **true**), `min_length` and
  `max_length`. Prompt lengths count characters, like JSON Schema's
  `maxLength`, so use the vendor's limit as written. String params
  (`negative_prompt`, `quality`, `style`, `resolution`) still check
  `max_length` in bytes.
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
  `upload_init_image` and PixVerse's image upload do.
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
- For image and video, these flags only feed `GET /v1/models` (and the
  landing site), and the validator ignores them. 3D gains a validator check
  for text-only requests. A model that cannot run without an image therefore
  needs its role `required: true, min_count: 1`. The adapter must also refuse a request
  whose roles imply an unsupported mode, with `ProviderError::InvalidRequest`,
  before any vendor call (vidu's endpoint selection, fal 3D's empty arms).
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

The allowlist is enforced only when `strict` is true (the default). With
`strict: false`, `check_extra` returns early and **the whole `extra` object
reaches the adapter unfiltered**. That is why adapters must not let `extra`
override fields they own (rule 11 in SKILL.md).

If users need a knob that the UI should render, or that more than one vendor
shares, it should become a canonical param. That is a core change, so ask the
user first.

## Pricing

- `base_cost_usd` is the vendor's price for the row's default request.
  `router.rs` bills it plus `cost_markup_percent`, multiplied by the images
  delivered for images, and once at submit for video and 3D (a job that
  later fails is still charged). `estimate_cost` should return the same flat
  number; OpenAI image's size and quality scaling is the known exception.
- Put the vendor's price string in a comment, quoted, with the arithmetic and
  `@see <pricing url> (YYYY-MM-DD)`:
  `# Default request (720p, 5 s): $0.055/s × 5 s = $0.275.`
- Record what the default doesn't cover in the comment, and in
  `variable_pricing` when it's a clean multiplier: higher resolution, longer
  duration, ×3 for a textured mesh, per-megapixel sizes, add-ons. Nothing
  applies `variable_pricing`; it is served in `GET /v1/models` for callers
  to read.
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
  `resolve_duration`) or reject with `InvalidRequest` before submitting.
  Test that behavior.
- **Per-mode value sets.** Intersect them, or split the model into two rows
  (see "One list serves every mode" above).
- **Coupled params**, such as "1080p only at 5 s" or "negative_prompt only in
  text-to-image". Enforce them in the adapter with `InvalidRequest`, or drop
  the field in the mode that forbids it, and document it in a YAML comment.
