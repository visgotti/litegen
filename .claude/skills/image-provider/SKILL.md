---
name: image-provider
description: Implement or update image generation for a litegen provider. Use it whenever the task touches the image family, such as adding an image vendor, adding image support to a vendor litegen already has for video or 3D, adding, updating, re-pricing or retiring an image model in models/*.yaml, acting on a /model-drift decision for an image model, or fixing an adapter in litegen-core/src/providers/image/. Use it even when the request only names a vendor or model ("add FLUX 3", "support Ideogram 5", "Recraft changed its sizes"). It covers the ImageProvider adapter, the catalog row, auth and dashboard credentials, registry wiring, tests, live verification and drift hooks, and hands off to video-provider or model3d-provider when the vendor does more than images.
---

# Image providers

**Read `.claude/skills/provider-standards/SKILL.md` first.** It holds the rules
every provider shares: auth and credentials, `api_base`, vendor-doc
citations, pricing, retries, tests and drift hooks. This skill adds only what
is specific to images. If the same vendor does, or will do, video or 3D, also
read [Vendors with more than one family](#vendors-with-more-than-one-family).

## Pick your path

| You're asked to | Path |
|---|---|
| Add an image model for a vendor litegen already serves images for | [A](#a-model-on-an-existing-image-adapter) |
| Update a carried image model (params, limits, price) | `provider-standards/references/touchpoints.md` section B, plus [Writing the adapter](#writing-the-adapter) if the request shape changes |
| Retire or rename an image model | `provider-standards/references/touchpoints.md` section C |
| Add image generation for a vendor litegen has for video or 3D | [B](#b-images-for-a-vendor-litegen-already-has) |
| Add a vendor litegen doesn't have yet | [C](#c-a-new-image-vendor) |
| Fix an image adapter | Write the failing wiremock or conformance test first, then fix the adapter, then work through [Done](#done) |

## How image generation works here

- **Contract.** `ImageProvider::generate(model, base, extras, materialized)`
  returns `GenerationOutput { data, content_type, metadata }`
  (`litegen-core/src/providers/mod.rs`). From the caller's side the call is
  synchronous: return the final image bytes. The router stores them (the
  app's S3 bucket gives a URL; without a bucket they go back inline as
  base64), answers in the caller's `response_format`, bills
  `base_cost_usd` × 1, and caches the result.
- **One image per call.** `GenerationOutput.data` holds exactly one image.
- **Inputs.**
  - `base`: prompt, `negative_prompt`, `seed`, `n`, `strict`, `extra`.
  - `extras: ImageExtras`: `size`, `aspect_ratio`, `quality`, `style`,
    `steps`, `guidance_scale`, `strength`, `response_format`, `extra`.
  - `materialized.refs`: `(role, form)` pairs, already in the row's
    `provider_format`.

  Each value is either absent or within the YAML's bounds.
- **Vendor flows you'll meet**, each with an example to copy:

  | Flow | Vendors | Pattern |
  |---|---|---|
  | Sync JSON returning base64 or a URL | OpenAI `b64_json`, ByteDance Seedream, MiniMax, Recraft | One POST, then decode or download |
  | Sync multipart | Ideogram, Stability v2, OpenAI `/images/edits` | `reqwest::multipart::Form`; refs arrive as `MultipartField` |
  | Async submit + poll, inside `generate()` | BFL (`polling_url`), Luma, Kling, Leonardo, Hunyuan, Runway, Replicate | Submit, then poll on a bounded loop, then download |

## A. Model on an existing image adapter

1. Read the vendor's API page, request schema and pricing for this model,
   and note the dates.
2. Write the conformance facts for the new row in
   `litegen-core/tests/catalog_conformance.rs`, and watch them fail.
3. Add the row to `models/<p>.yaml`, starting from the template in
   `references/adapter-template.md`: `docs_url`, `@see` comments, a pricing
   comment, and params, roles and capabilities exactly as the schema allows.
4. In `providers/image/<p>.rs`, add a resolution arm (or confirm the
   verbatim pass-through works) carrying the `@see` triplet. Compare the
   model's schema field by field with its siblings'. Any difference (a
   renamed field, a missing field, a different ratio enum) needs per-model
   gating and a wiremock test.
5. Map the id in `scripts/model-drift/providers/<p>.mjs`, and check that a
   typings source snapshots its schema.
6. Run the checks in [Done](#done).

## B. Images for a vendor litegen already has

Here the vendor is already integrated for video or 3D, and you're adding
the image family.

1. Read the vendor's existing adapters and YAML first. Reuse its provider
   name, `AuthSpec`, credential handling, `api_base` default (when one host
   serves both families) and env vars. Don't create a second provider name
   or a new env var for the same key.
2. Create `providers/image/<p>.rs` and add `pub mod <p>;` to
   `providers/image/mod.rs`. Any logic both families need (id translation,
   uploads, error-envelope parsing, host or region selection) goes into
   `pub(crate)` functions that the other adapter imports. That is how
   `video/bytedance.rs` uses `image::bytedance::ark_model_id`. Auth helpers
   go in `providers/auth/`.
3. In `proxy/registry.rs`, add the import, an arm in `build_image_provider`,
   and the name in `IMAGE_PROVIDERS`. The dashboard's credential form gains
   "image" with no UI change.
4. Add the image rows to the vendor's existing `models/<p>.yaml`, under an
   image section. If the vendor reuses model names across families, give
   one family a prefix so ids stay unique.
5. Test that the vendor's stored dashboard credential configures the new
   adapter:
   `registry.image_provider_for_request("<p>", Some(ProviderCredentials::from_json(&json!({"api_keys":[{"key":"k"}]})))).await`
   must return an instance that is configured.
6. Do everything in path A for each model, then wire the rest from
   `provider-standards/references/touchpoints.md` section D: the drift
   sources for the image endpoints, the landing flags and syncs, the README
   and a live test.

## C. A new image vendor

1. Confirm the vendor has a self-serve, first-party API, and note how keys
   are obtained. Pick its auth scheme from
   `provider-standards/references/auth-and-credentials.md`. If none fits,
   that is a new `AuthSpec` variant and needs its own tests.
2. Choose the provider name: lowercase and stable. It becomes the id
   prefix, the env-var stem and the dashboard entry.
3. Build the adapter from `references/adapter-template.md`, with its tests
   written first.
4. Wire everything in `provider-standards/references/touchpoints.md`
   sections D and E: the registry arm and list, `provider_catalog()` (only
   for non-API-key credentials), the env-var mapping in `config/mod.rs`,
   `litegen.example.yaml`, `.env.example`, a new drift definition, the
   landing and README.
5. If the vendor also does video or 3D, decide with the user whether that is
   in scope now. Either way, build the image half so the other half can
   reuse it (see [the section below](#vendors-with-more-than-one-family)).

## Writing the adapter

The full skeleton is in `references/adapter-template.md`. What follows is
specific to images; the shared rules still apply.

**Request**

- **Choose the endpoint and mode from the refs.** An `init` or `mask` ref
  means image-to-image or inpainting, which many vendors serve on a separate
  endpoint (OpenAI sends refs to multipart `/v1/images/edits`). Send each
  mode's fields only in that mode. Stability takes `mode: image-to-image`
  and `strength` only with an init image, and takes `aspect_ratio` only for
  text-to-image.
- **Decide how size and aspect ratio interact, explicitly.** If a row
  declares both, document which wins (BFL uses `size` as width and height,
  and falls back to `aspect_ratio`). If the vendor takes only ratios, don't
  declare `size` unless you derive the ratio through a named enum table
  with an exhaustive test (`resolve_ratio_only_ever_returns_enum_members`).
  Pixel constraints such as multiples of 16 or bounds belong in
  `size: { kind: size, mode: freeform, multiple_of, … }`. A fixed list belongs
  in `size: { kind: size, mode: enum, values: [[w, h], …] }`.
- **Param names vary by model, even within one vendor.** On Replicate, FLUX
  takes `guidance` and SD3 takes `cfg`. fal uses `num_inference_steps`,
  Recraft `random_seed`, Stability `style_preset` and Ideogram `style_type`.
  gpt-image takes `quality` values of `low|medium|high|auto`. Gate each
  field per model.
- **`response_format` is litegen's.** It decides URL or base64 in *our*
  response. Forward it only when the vendor field means the same thing for
  that model; gpt-image rejects `response_format`.
- **Never forward `n`.** You return one image, and the vendor would bill
  for N.
- **Ask for an output format you can name.** BFL sends
  `output_format: png`, so it can truthfully claim `image/png`. Otherwise
  read the vendor's declared format or its `Content-Type`.

**Async vendors (polling inside `generate()`)**

- Use the polling URL the vendor returns (BFL `polling_url`) when there is
  one, but send your key to it only if its host is the vendor's API host or
  the `api_base` host. Otherwise build the poll URL from `api_base()`.
- Give `generate()` one wall-clock deadline, about 90 s from entry. It must
  cover the submit, every poll round trip and the download, not just the
  sleeps, because the router's 120 s timeout measures the whole call.
  Existing adapters allow 180–240 s, which is a known gap.
- Once the vendor has accepted the job, every error is non-retryable. That
  includes exhausting your own poll budget: return a non-retryable
  `RequestFailed` that carries the vendor job id, not `ProviderError::Timeout`.
  The router retries a `Timeout`, and that re-submits a job you've already
  paid for.
- Map moderation and failure statuses to non-retryable errors that carry
  the vendor's reason (BFL `Content Moderated`, `Request Moderated`,
  `Error`).

**Output**

- Download result URLs inside `generate()`, since signed URLs expire (BFL's
  last about ten minutes). Check the HTTP status and cap the size. For
  base64 responses, decode with the standard engine after
  `strip_data_prefix`.
- Make `content_type` exact. The S3 store derives the object's extension
  from it, mapping png, jpeg and webp and storing anything else as `.png`.
  Vector rows return `image/svg+xml`.
- In `metadata`, only `"revised_prompt"` reaches the caller; the router reads
  it. Other entries, such as the vendor job id or the seed used, are for
  logs. Never put credentials in them.

## The catalog row

A full annotated template is in `references/adapter-template.md`. The parts
specific to images:

- **`capabilities`** are some of `text_to_image`, `image_to_image` and
  `inpainting`, whichever the adapter actually implements.
- **Roles**: `init` (image-to-image source), `mask` (inpainting), plus the
  vendor's reference kinds: `style`, `character`, `subject`, `modify`,
  `redux`. A model that cannot run without a reference marks the role
  `required: true, min_count: 1`, as Runway `gen4_image_turbo` does.
- **Form**: `base64` wherever the vendor accepts it. A `url` row receives a
  non-fetchable value when the caller sends base64 or a blob (see
  known-gaps.md). Use `multipart` with a `field_map` for multipart endpoints,
  and if the vendor validates a part's content type, set it from the bytes
  with `sniff_image_mime`.
- **Declare a pixel `size` only if the vendor takes width × height for that
  model.** Recraft's V4.1 and V2 vector rows take ratios only and declare no
  `size`, while `recraft/recraftv3_vector` does take WxH.
- **Price** is the vendor's price for one image at the default size and
  quality. Per-megapixel vendors (FLUX on BFL and fal) cost more at larger
  sizes, so document the multiplier and raise wide spreads with the user.

## Tests

Write these in the adapter's `#[cfg(test)] mod tests`, using the idioms in
`provider-standards/references/testing.md`:

- [ ] **Happy path for each request shape** (text-to-image, image-to-image,
      inpaint, each distinct endpoint): method, path, auth header, every
      body field name and value, and decoded bytes plus `content_type`.
- [ ] **Each declared param** produces the vendor's field name and value
      on the wire. Each field an endpoint lacks is absent, with the input
      actually set.
- [ ] **Refs**: each role reaches the right field with the right encoding,
      the endpoint switches when `init` or `mask` is present, and
      mode-only fields appear only in their mode.
- [ ] **A pool-only credential** (`api_keys`, no `api_key`) authenticates
      the request.
- [ ] **Async vendors**: poll → ready → download works; failed and
      moderated statuses give a non-retryable error carrying the vendor's
      reason; a non-2xx download gives an error.
- [ ] **Errors**: a vendor 4xx gives a non-retryable error whose message
      contains the vendor's text. A 5xx or 429 on submit is retryable.
- [ ] **Resolution ↔ catalog**: every `media_type: image` row for this
      provider resolves. Iterate `CapabilityRegistry::for_provider`; don't
      copy the list.
- [ ] **Derivations** (size → ratio and similar) return only vendor enum
      members across every input.
- [ ] **Conformance facts** for every enum, bound, prompt limit and
      required role in the YAML.
- [ ] **A live test** `live_<p>_image` in `tests/live_providers.rs`, run when
      credentials and a go-ahead exist. Check that the bytes decode as the
      claimed type.

## Vendors with more than one family

Most vendors here serve several families: kling, minimax, luma, runway,
bytedance, leonardo, hunyuan, google, fal and replicate. Run
`grep -h media_type models/<p>.yaml | sort | uniq -c` to see a vendor's
families today.
The customer configures one key in the dashboard and expects it to work for
every family, so the families must agree.

- **The vendor already has video or 3D in litegen.** Follow path B. If
  you're also changing the video half, do that half with the
  `video-provider` skill, and the 3D half with `model3d-provider`. Don't
  improvise a family's adapter from this skill.
- **The vendor will get video or 3D later.** Build for it now:
  - put vendor-shared logic in `pub(crate)` functions rather than inside
    the `ImageProvider` impl;
  - choose image ids that can't collide with future video ids;
  - write the drift definition so the other family's sources can be added
    to the same file;
  - name the planned family in a header comment in `models/<p>.yaml` and in
    your report, so whoever builds it starts from `video-provider`
    path B.
- **Keep the halves consistent.** Use the same `AuthSpec`, the same
  `api_base` default and meaning, the same credential and env var, and the
  same error-envelope parsing. If the sibling adapter is legacy (it
  hand-writes its auth header, for example), match the standard rather than
  the sibling. Mention the mismatch in your report, or migrate the sibling
  behind a test if the user agrees.

## Done

- [ ] Everything under "Before you report" in
      `provider-standards/references/testing.md`.
- [ ] The touch points for your change, checked against
      `provider-standards/references/touchpoints.md` (whichever of sections
      A–E applies).
- [ ] Any new image param × provider cells added to
      `docs/superpowers/param-mapping-matrix.md`.
- [ ] A report in the shape given under Finish in provider-standards. Name
      every request shape that was not verified live.
