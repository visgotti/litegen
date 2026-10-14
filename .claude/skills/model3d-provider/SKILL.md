---
name: model3d-provider
description: Implement or update 3D mesh generation (the model3d family) for a litegen provider. Use it whenever the task touches 3D, such as adding a 3D vendor (Meshy, Tripo, Hyper3D Rodin, Stability SF3D/SPAR3D, Tencent Hunyuan3D, or the 3D endpoints of fal, Replicate and other aggregators), adding, updating, re-pricing or retiring a model3d row in models/*.yaml, output formats (glb, obj, stl, ply, fbx, usdz), textures and PBR, image-to-3D or multiview inputs, or fixing litegen-core/src/providers/model3d/. Use it even when the request only names a vendor or model ("add Meshy 6", "TRELLIS 2 on fal", "Rodin Gen-2.5"). It covers the Model3dProvider adapter (submit, stage_context, polls that return downloaded bytes), the output_formats promise and local conversion, the catalog row, auth and dashboard credentials, registry wiring, tests, live structural verification and drift hooks, and hands off to image-provider or video-provider for vendors that also do those.
---

# 3D model providers

**Read `.claude/skills/provider-standards/SKILL.md` first.** It holds the rules
every provider shares: auth and credentials, `api_base`, vendor-doc
citations, pricing, retries, tests and drift hooks. 3D is the newest family
and the strictest one. Every completed generation is a promise that three
separate observers check, every output is re-hosted, and vendor bytes are
parsed as untrusted input. Read this whole skill before writing code.
`references/vendor-notes.md` holds what's known about each 3D vendor.

## Pick your path

| You're asked to | Path |
|---|---|
| Add a 3D model on an existing 3D adapter (today only `model3d/fal.rs`) | [A](#a-model-on-an-existing-3d-adapter) |
| Update a carried 3D model (formats, params, price) | `provider-standards/references/touchpoints.md` section B, plus [Writing the adapter](#writing-the-adapter) if the request shape changes |
| Retire or rename a 3D model | `provider-standards/references/touchpoints.md` section C |
| Add 3D for a vendor litegen already has (stability, hunyuan, replicate…) | [B](#b-3d-for-a-vendor-litegen-already-has) |
| Add a new 3D vendor (meshy, tripo3d, rodin…) | [C](#c-a-new-3d-vendor) |
| Fix a 3D adapter | Write the failing test first (wiremock, conformance or the 3D API suite), then fix the adapter, then work through [Done](#done) |

## How 3D generation works here

- **The contract** lives in `litegen-core/src/providers/mod.rs`:
  - `Model3dProvider::generate(model, base, extras, materialized)` submits
    the job and returns a `Model3dGenerationHandle { provider_job_id,
    provider, model, stage_context }`.
  - `poll_status(handle)` returns `Model3dGenerationPollResult { status,
    progress, files, error, metadata }`.
  - `files` holds **downloaded bytes**: `Model3dFile { kind:
    Mesh|Texture|Preview, format, content_type, bytes, polycount, width,
    height }`.
- **The mode comes from the refs**, never from a param: no refs means
  text-to-3D, an `init` ref means image-to-3D, and `view-front|back|left|
  right|top` refs mean multiview.
- **`generate()` only submits.** It runs under the router's 120 s attempt
  timeout and up to two retries, and each retry is a new billed vendor job.
- **`stage_context` is your polling state.** Put the endpoint, the vendor's
  status and result URLs, and the job's mode there. The handler persists it
  to the row as `metadata.stage_context` **at submit time only**, and the
  poller reads it back after a restart. `poll_status` gets `&handle`, and no
  code path persists a *changed* context, which matters for multi-stage
  vendors (see [below](#multi-stage-and-synchronous-vendors)).
- **Two observers.** Client polls (`GET /v1/models3d/{id}`) go through the
  router, reusing the instance that submitted the job, and fall back to the
  row on error. The background poller builds a fresh instance with the org's
  BYO credential, possibly in another process or after a restart.
- **The completion pipeline is core code; don't duplicate it in the adapter.**
  When you return `Completed` with files, core runs three steps:
  1. `synthesize_missing_formats` derives any requested glb, obj, stl or ply
     from a mesh it can read. It runs on a blocking thread and never
     replaces a vendor file.
  2. `rehost_model3d_files` uploads every file to the app's BYO bucket or
     the global store, under
     `{prefix}/{generation_id}/{model|texture|preview}[_n].{ext}`, all in
     the same tick.
  3. The guard checks that every promised container was delivered. If any is
     missing, the generation becomes `failed` and the webhook fires. The
     poller, `get_3d_status` and the row read-back all enforce this.
- **Billing** is a flat `base_cost_usd` per generation.
- **The consumer contract.** aipix, the first 3D consumer, coded against
  wire shapes that are now fixed:
  - media type `model3d` and routes under `/v1/models3d/…`;
  - a completed generation carries one mesh asset per promised container,
    all with absolute URLs. `mesh()` and `result_url` prefer glb when it's
    present, but a request for `[fbx]` alone completes with no glb;
  - `strict: false` drops unsupported params and reports them in
    `X-Litegen-Dropped-Params`;
  - params are first-class and carry a `label` and `description`.

  Its contract lives in the aipix repo and is summarized in
  `docs/superpowers/specs/2026-08-19-3d-model-generation-design.md` §14.1.
  A provider task doesn't change wire shapes.

## The `output_formats` promise

`output_formats` is unlike every other param. Other params are hints a
vendor may ignore; this one is a **promise**, enforced after the vendor has
been paid.

- **In YAML** it is `kind: string_array`.
  - `enum_values` lists what the **vendor emits natively** for this
    endpoint, verified in its schema. It must include `glb`, which the loader
    enforces, and use the canonical spellings in `Model3dFormat`
    (`types/mod.rs`; on 2026-10-12: glb, gltf, obj, stl, ply, fbx, usdz,
    usda, 3mf, dae).
  - `max_items` is the number of **vendor jobs** one generation can make,
    not how many formats a caller may name. A scalar format field means
    `max_items: 1`.
  - `default` is `[glb]`.
- **The API advertises** what can be delivered: the native formats, plus
  every container litegen can derive, as long as at least one native format
  is readable. It also exposes `derivable_values`.
  - The derivable set is `model3d_convert::derivable_formats()`. Since the
    mesh conversion work landed (2026-10-12), it is all ten `Model3dFormat`
    containers. `docs/superpowers/3d-format-conversion.md` describes what
    each conversion preserves. Read the function rather than assuming the
    set.
  - So a promised container now goes missing only when the vendor's own file
    can't be read: a Draco-compressed GLB, for example (see below).
    `mock/partial-3d` models exactly that case.
- **Validation** checks that every requested format is deliverable, and that
  `vendor_jobs_needed(requested) ≤ max_items`. A non-derivable format costs
  one job, and all the derivable ones together share a single source job.
  A format `prefers_vendor_native` marks but that is also derivable still
  counts as derivable for this budget. So Rodin, with `max_items: 1`, serves
  `[glb, fbx]` from ONE vendor job: its own fbx, chosen through
  `prefers_vendor_native`, with the glb derived from that fbx.
- **The adapter asks the vendor for exactly one container.** Choose it with
  `model3d_convert::prefers_vendor_native(format)`, never by testing
  derivability yourself. That function returns true for anything litegen
  can't derive, and *always* for fbx and usdz, because a vendor's own export
  carries rigs and materials that a conversion from glb doesn't.
  - First choice: a requested format it returns true for, when the endpoint
    offers it.
  - Otherwise `glb`, when offered: it's the default conversion source.
  - Otherwise the first requested format the endpoint offers, then the
    endpoint's default.

  An endpoint with no format field gets no field at all; an undefined field
  is a 422, not a no-op. `vendor_format` in `model3d/fal.rs` implements
  exactly this. Its tests exist because the first version got it wrong and
  was billed for it. On 2026-10-12 they were
  `the_vendor_is_asked_for_glb_unless_a_format_only_it_can_make_was_promised`
  and `rodin_is_asked_for_its_own_fbx_rather_than_a_conversion`.
- **Return what came back**, tagged with its real format. Never convert,
  rename or fabricate a container. Conversion is core's job, and a
  plausible-but-broken file surfaces at the customer instead of at us.
- **Derivation needs a vendor file that litegen can read.** The glb reader
  refuses any `extensionsRequired` entry outside its allowlist, and Draco,
  meshopt and `KHR_mesh_quantization` are all refused. If a vendor's file
  can't be read, every promised container except the one it delivered fails
  *after* the vendor has billed. Prove in the live run that the real vendor
  file parses and converts. The catalog can't yet express "this vendor's
  output can't be converted", so if the file doesn't parse, stop and raise
  it.

## A. Model on an existing 3D adapter

1. Read the endpoint's own schema (for fal:
   `https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={slug}`), its
   API page and its pricing (fal: `https://fal.ai/models/{slug}/llms.txt`),
   and note the dates. Fields differ sharply between endpoints. The format
   param alone is spelled four ways on fal, and most endpoints have none.
2. Write the conformance facts first, in
   `litegen-core/tests/model3d_conformance.rs` (or `catalog_conformance.rs` if
   that file is where 3D facts live in your checkout). That file already
   requires `docs_url` on every non-mock 3D row. Watch the new facts fail.
3. Add the row (template in `references/adapter-template.md`): `docs_url`,
   the `@see` schema and pricing lines with the quoted price, native-only
   `output_formats`, `capabilities` that match the roles, the seed range
   from the schema, and vendor knobs in `extra_allowlist`.
4. Add an endpoint-spec arm with an `@see` triplet (API page, schema,
   pricing), listing:
   - the text and image endpoints, empty where a mode doesn't exist;
   - the image field name and whether it's an array;
   - the format field and its values;
   - the seed ceiling;
   - the texture and material fields.

   For a fal slug, also add the id to `fal::MODELS`. Two tests tie it to
   the catalog in both directions:
   - `every_fal_3d_row_is_served_here_and_every_arm_is_in_the_catalog`;
   - `each_catalog_row_declares_exactly_what_its_endpoint_takes`. It checks
     that `output_formats` matches the endpoint (or is `[glb]`), the seed
     range, `texture`/`pbr` exactly when the endpoint has those fields, an
     `extra_allowlist` that excludes mapped fields, the `url` form, `init`
     required exactly when there is no text arm, a `docs_url` containing the
     endpoint, and `prompt.max_length: 0` exactly when there is no prompt
     field.
5. Map the id to every endpoint it uses in the drift definition, and make
   sure a typings source covers each one.
6. Run the checks in [Done](#done), and prove the row live before you claim
   any format beyond glb.

## B. 3D for a vendor litegen already has

Here the vendor is already integrated for images or video (Stability's
SF3D and SPAR3D use the `stability` key, for example), and you're adding
3D.

1. Reuse the vendor's provider name, `AuthSpec`, credential handling,
   `api_base` default and env vars. Stability 3D is `stability`, not a new
   name. The customer's existing key must just work.
2. Create `providers/model3d/<p>.rs` and add `pub mod <p>;` to
   `providers/model3d/mod.rs`. Import the vendor's shared `pub(crate)`
   helpers, or move shared logic into them. Signing helpers (TC3 for
   Hunyuan3D) already live in `providers/auth/`.
3. In `proxy/registry.rs`, add the import, an arm in
   `build_model3d_provider`, and the name in `MODEL3D_PROVIDERS`.
   `provider_catalog()` then reports the `model3d` modality, and the
   dashboard form and `/health` both update by themselves.
4. Add the rows to the vendor's YAML file in a 3D section.
5. Test that the stored dashboard credential configures the new 3D adapter
   (`registry.model3d_provider_for_request("<p>", Some(…)).await`).
6. Do everything in path A for each model, then wire the rest from
   `provider-standards/references/touchpoints.md` section D: the drift
   sources, the landing (there is no 3D flag in `site.ts`;
   `FALLBACK_CAPABILITIES` has `model3d`), the README and a live run.

## C. A new 3D vendor

1. Check `references/vendor-notes.md` and confirm against the vendor's live
   docs. Settle these questions before writing code:
   - Is access self-serve? (Rodin direct is a Business-tier purchase.)
   - Is generation sync or async?
   - Is it single-stage or multi-stage?
   - Which formats come out natively?
   - Are textures inside the mesh or separate?
   - When do download URLs expire?
2. If the vendor is multi-stage or synchronous, read
   [Multi-stage and synchronous vendors](#multi-stage-and-synchronous-vendors)
   first. Either may need a core change, and that is the user's call.
3. Build the adapter from `references/adapter-template.md`, tests first.
4. Wire everything in `provider-standards/references/touchpoints.md`
   sections D and E.

## Writing the adapter

**Submit**

- **Use one endpoint-spec table per model** (the fal pattern) whenever
  endpoints differ in field names, arity or ranges. Read each spec from that
  endpoint's own schema; don't share a vocabulary across endpoints.
- **Choose the endpoint from the refs.** A missing mode is an empty arm, and
  the adapter answers it with `InvalidRequest` before any vendor call.
  TRELLIS and Hunyuan3D require an image, so a text-only request is refused,
  and the YAML marks `init` as `required`.
- **Image refs** go in the vendor's form: a URL, a base64 data URI (Meshy
  accepts one), a vendor file token after an upload (Tripo), or multipart
  (Stability, Rodin direct). If a ref can't be sent, refuse the request
  rather than submitting without it, because the caller paid to upload it.
  A `url`-form row reliably works only with caller-supplied public URLs. Base64
  and blob refs arrive as a `local://` URL or an inline `data:` URI, depending
  on the materializer version (known-gaps.md), so check whether the vendor
  accepts `data:` URIs.
- **Canonical params map to vendor fields:**

  | litegen param | Vendor field |
  |---|---|
  | `texture` | the vendor's texture toggle (Hunyuan3D `textured_mesh`) |
  | `pbr` | the vendor's material field (Rodin `material: PBR\|Shaded`: off means flat shading, not "no material") |
  | `target_polycount` | Meshy `target_polycount`, Tripo `face_limit`, Stability `vertex_count` |
  | `topology` | Meshy `topology`. Tripo's `quad` *forces FBX output*, so map it only together with that format side effect. |
  | `symmetry` | Meshy only, and Meshy deprecated it. Don't add it to new rows. |

- **Refuse an out-of-range seed; don't clamp it.** Rodin's seed is a u16, so
  a larger value is a 422 at the vendor, and clamping would stop `seed`
  meaning "reproducible".
- **Merge `extra` before the fields you own** (rule 11). Vendor-specific
  knobs (Rodin `tier`, TRELLIS `mesh_simplify`, Hunyuan `octree_resolution`)
  are `extra_allowlist` passthroughs.
- **Return the handle** with `stage_context` set to `{endpoint,
  status_url, response_url, …}`.

**Poll**

- **Map statuses, including the vendor's quirks.** fal reports a failure as
  `COMPLETED` plus `error` (with `error_type`); its queue enum is closed, so
  an unknown status is an error. Rodin's state is `Done`. Tripo has its own
  vocabulary. Read the docs for each vendor.
- **On terminal success, find every mesh URL**: fal's `model_mesh` or
  `model_glb`, Meshy's per-format `model_urls`, or Rodin's separate download
  call. **Download in the same call** (Tripo URLs expire about 5 minutes
  after success):
  - cap the size before buffering (`MAX_ASSET_BYTES`, 128 MiB in fal) and
    again after;
  - check the HTTP status;
  - send no API credentials to CDN hosts.
- **Work out each file's format** from the URL's extension first (object
  stores label everything `application/octet-stream`), then the content
  type, then `glb`.
- **Fill `files`:**
  - one `Mesh` per container the vendor delivered;
  - `Texture` for separate maps (Rodin's `textures` array, Meshy's
    `texture_urls`);
  - `Preview` for thumbnails;
  - `polycount` only if the vendor reports it. Never echo the request's
    `target_polycount`.
- **Completed without a mesh, or with an error, is `Failed`** with the
  vendor's own message. The guard would fail it anyway, but this way the
  vendor's reason reaches the caller.
- **A texture or preview that won't download shouldn't fail a good mesh.**
  Skip it with a `warn!`. The mesh is the deliverable.
- **Verdicts are `Ok(Failed)`; `Err` means "we don't know yet"** (from the
  trait docs on `Model3dGenerationPollResult`).
  - A vendor failure, or a job that finished but whose result can no longer
    be fetched (an expired or gone asset URL), is
    `Ok(… status: Failed, error: Some(reason) …)`. That takes the terminal
    path and fires the webhook.
  - A transient poll or download error (5xx, 408, 429, timeout) is a
    retryable `Err`, and the poller retries the whole poll next tick.
  - A non-retryable `Err` is only for a state you can never learn, such as
    a 401 or 404 on the status call. The poller reaps those rows *without* a
    webhook.

  `fetch_asset` in `model3d/fal.rs` returns `AssetError::{Transient, Gone}`
  so that its caller can map each case correctly.
- **Progress** is the vendor's percentage when given. fal reports only queue
  position, so it uses a fixed 50 rather than inventing a curve.

**Cancel**

- `Model3dProvider::cancel(handle)` is a best-effort hook. When a caller
  cancels, the row is already `cancelled`, and this call only decides
  whether the *vendor* keeps working and billing. If the vendor has a cancel
  endpoint (fal does), implement it, and return `Requested`,
  `AlreadyFinished` or `NotFound` (`CancelOutcome` in `providers/mod.rs`).
  Otherwise keep the default `Unsupported`. A synchronous vendor's adapter
  can drop the stashed bytes and answer `AlreadyFinished`.

## Multi-stage and synchronous vendors

- **Multi-stage** (Meshy text-to-3D: `preview`, then `refine` referencing
  `preview_task_id`). `stage_context` is **write-once**: it is persisted at
  submit, and nothing persists a change made later. See the doc comment on
  `Model3dGenerationHandle.stage_context` and the open items in spec §14.5.
  An adapter that starts stage two from `poll_status` loses that job id on a
  restart. Worse, the client's poll and the poller can each start, and each
  bill, their own stage two. A multi-stage vendor needs core plumbing built
  first:
  1. a poll result that carries the new stage state;
  2. both observers of completion (`get_3d_status` and the poller)
     persisting it;
  3. an atomic claim, so only one observer can advance the stage.

  That is design work, and it belongs to the user. Until it exists, offer
  only the vendor's single-stage flows (Meshy image-to-3D folds texturing
  into one task). An honestly labelled preview-only, untextured text row is
  also fine.
- **Synchronous** (Stability SF3D and SPAR3D return the GLB in the response
  body). The trait is submit-and-poll, so the adapter calls the vendor inside
  `generate()`, keeps the bytes, and returns a handle whose poll returns
  `Completed` with those bytes. Getting this right:
  - **Use a process-wide store.** Make it a `static` (`OnceLock<RwLock<HashMap<…>>>`,
    like `jobs()` in `model3d/mock.rs`), never a field on the adapter. The
    submit, the client polls and the poller can each run on a different
    instance, and the poller builds a fresh one every tick.
  - **Keep each entry until a sweep removes it,** because both observers may
    ask. Bound the store with a TTL and a total byte cap.
  - **Set the client timeout under the router's 120 s attempt,** and make
    every failure after the request is sent non-retryable. The vendor may
    already have billed, and a router retry is a second billed call.
  - **On a miss** (a restart, or another replica holding the bytes, since each
    process runs its own poller), return a *retryable* error for a short
    grace window after submit. Put `submitted_at` in `stage_context` so you
    can tell. After the window, return `Failed` with an explicit message
    ("result lost before storage; the vendor billed it; resubmit"). Never
    leave it `Pending` forever.

  A restart in that window loses a billed result. Raise this with the user
  before you build. The durable alternative is a core change: re-hosting at
  submit time.

## The catalog row

A full annotated template is in `references/adapter-template.md`. The parts
specific to 3D:

- **`capabilities`** are some of `text_to_3d`, `image_to_3d` and
  `multiview_to_3d`. Each needs its roles: `image_to_3d` needs an `init` role,
  and multiview needs at least two `view-*` roles
  (`every_3d_model_advertising_image_modes_declares_the_matching_ref_roles`).
  A 3D row may not declare `size`, `fps`, `duration_seconds` or
  `aspect_ratio`.
- **Prompt.** Use `prompt.required: false` when the image modes need no
  prompt, and take `max_length` from the schema. An endpoint with no prompt
  field at all (TRELLIS) declares `max_length: 0`: strict callers then get
  `param_unsupported` and lax callers have the prompt dropped, so a prompt
  never reaches a schema that lacks the field.
- **`docs_url`** is required, along with the `@see` schema and pricing
  lines.
- **Price** is the vendor's flat per-generation price. Put conditional
  multipliers in `variable_pricing` (Hunyuan3D `textured_mesh_multiplier: 3`,
  Rodin `addons_highpack_multiplier: 3`), and raise them with the user,
  because flat billing under-charges textured or add-on requests.
- **`display_name`** names an aggregator host: "TRELLIS (Fal)".

## Tests

Write these in the adapter's `#[cfg(test)] mod tests`, using the idioms in
`provider-standards/references/testing.md`:

- [ ] **Resolution ↔ catalog, both ways**: every `media_type: model3d` row
      for the provider resolves to an endpoint, and every arm in the
      adapter has a catalog row. Iterate the catalog; don't copy the list.
      fal's `every_fal_3d_row_is_served_here_and_every_arm_is_in_the_catalog`
      together with its `pub const MODELS` is the pattern.
- [ ] **Vendor format choice**:
  - a format `prefers_vendor_native` selects (fbx, usdz, or anything
    litegen can't derive) is asked of the vendor;
  - derivable requests ask for glb when it's offered;
  - an endpoint without a format field gets no field.
- [ ] **Submit**:
  - each endpoint's path and auth (one test uses a pooled key);
  - the image field's spelling and arity per endpoint;
  - texture and material mapping;
  - the seed ceiling, refused when out of range;
  - a text-only request to an image-only row refused with no HTTP call;
  - the `stage_context` contents.
- [ ] **Poll**:
  - success downloads the mesh (wiremock serves a small GLB) and tags
    `kind`, `format` and `content_type`;
  - success with an error → `Failed` carrying the vendor message;
  - success with no mesh → `Failed`;
  - a texture that fails to download is skipped, not fatal;
  - an oversized asset is refused;
  - a handle rebuilt from the row (`provider_job_id` + `stage_context`)
    polls correctly.
- [ ] **Format detection**: the URL extension wins over the content type, a
      query string isn't mistaken for an extension, and the fallback is
      `glb`.
- [ ] **Error envelopes**: every shape the vendor documents gives a usable
      message (fal has three).
- [ ] **Conformance facts**: `output_formats` native values and `max_items`,
      the seed range, required roles, mode capabilities and `docs_url`.
- [ ] **Core suites still pass**: `cargo test --test model3d_api --test
      model3d_validation`, and `--lib proxy::poller` if you touched
      anything near them.

## Live verification (the bar for 3D)

The 3D bar is higher, because a wrong claim costs a billed generation that
then fails. The 2026-09-12 fal runs set the standard
(`docs/superpowers/param-mapping-matrix.md`, 3D section):

- Put live tests in `litegen-core/tests/live_model3d.rs`. Its fal TRELLIS
  test submits, polls, and runs the downloaded GLB through
  `synthesize_missing_formats`; start from it. It's `#[ignore]`d and billed;
  run it with
  `cargo test --manifest-path litegen-core/Cargo.toml --test live_model3d -- --ignored --nocapture`.
- Run one real generation per endpoint and mode you added. Ask for every
  format you advertise, including any fbx or usdz.
- **Download every asset and check its structure**, not just that it's
  non-empty:
  - glb: `glTF` magic, version 2, declared length equal to actual length;
  - obj: no `mtllib`/`usemtl`;
  - stl: `84 + 50n` bytes, with a header that doesn't start with `solid`;
  - ply: `binary_little_endian`, with header counts that match;
  - fbx: the `Kaydara FBX Binary` header.

  Triangle and vertex counts must agree across derived formats.
- **Render-verify the vendor's real files.** Save what the vendor returned
  to a directory, then run
  `cargo run --example mesh_matrix -- /tmp/m --fixtures none --import <dir>`
  from `litegen-core`. Next, run `node verify.mjs /tmp/m /tmp/report` in
  `scripts/mesh-verify` (after `npm ci`). That renders the vendor's original
  with its own loader, compares it with litegen's reading, and checks every
  derived container with independent readers. `scripts/mesh-verify/README.md`
  explains the report.
- Let one job finish through the **background poller**, with no client
  polling.
- Fire each new validation rejection and confirm it costs $0 (no vendor
  call).
- Record the date, cost, what was proven and what wasn't in the 3D section
  of the param-mapping matrix. If you couldn't run it, write "not verified
  live", plainly, in the report.

## Vendors that do more than 3D

- **The vendor already has images or video in litegen.** Follow path B. Do
  any image or video work with the `image-provider` or `video-provider`
  skill.
- **Some 3D vendors also do images** (Stability, Tencent), and aggregators do
  everything (fal, Replicate). Keep one provider name, one `AuthSpec`, one
  credential and env var, and one drift definition per vendor.
- **If the sibling adapter is legacy** (it hand-writes auth or hard-codes
  hosts outside `api_base()`), match the standard, not the sibling, and
  report the mismatch.

## Done

- [ ] Everything under "Before you report" in
      `provider-standards/references/testing.md`, including
      `model3d_conformance`.
- [ ] The touch points for your path, checked against
      `provider-standards/references/touchpoints.md` (whichever of sections
      A–E applies).
- [ ] The 3D rows of `docs/superpowers/param-mapping-matrix.md` updated:
      vendor columns, the proof, and the live-run record.
- [ ] Your model's column added to the canonical-vocabulary table in the
      module docs of `litegen-core/src/providers/model3d/mod.rs`. Fill a cell
      only where the vendor's schema has the field and a test asserts the
      outbound name.
- [ ] A report in the shape given under Finish in provider-standards. Say
      exactly which formats were proven live and which only exist in the
      schema.
