---
name: provider-standards
description: The shared rules every litegen provider integration follows — dashboard/BYO credentials and AuthSpec auth, the single api_base URL, vendor-doc references (docs_url, @see with verbatim quotes), catalog YAML and pricing conventions, error/retry and billing semantics, required tests and live verification, and the model-drift hooks. The image-provider, video-provider and model3d-provider skills load this first. Use it directly for work that cuts across families or adapters - changing a provider's auth or credential fields, retiring or renaming models, auditing or reviewing an adapter against its vendor's API docs, or explaining what adding a provider involves.
---

# Provider standards

litegen puts one API in front of many generation vendors. An integration is
correct only when four things agree: what the vendor's API accepts, what
`models/*.yaml` promises, what the adapter sends, and what the tests prove.
Most defects found here so far were one of those four drifting from the
others (`docs/superpowers/2026-08-16-provider-api-audit.md`, "Recurring defect
patterns"). These rules keep them together.

The family skills add the specifics: `image-provider`, `video-provider` and
`model3d-provider`, all under `.claude/skills/`.

## The request path you plug into

```text
POST /v1/{images|videos|models3d}/generations
  → Validated{Image,Video,Model3d} extractor          api/middleware/validator.rs
      prompt spec, each param vs the row's ParamSpec, ref roles, extra_allowlist
      strict=true → 4xx param_unsupported · strict=false → drop + X-Litegen-Dropped-Params
  → materializer: refs → url | base64 | multipart    proxy/materializer.rs
  → {Image,Video,Model3d}Extras                       api/handlers/mod.rs
  → org BYO credential, or the platform instance      proxy/registry.rs
  → router: 120 s per attempt, up to 2 retries,       proxy/router.rs
      bills pricing.base_cost_usd (images: × images delivered;
      video and 3D: once, at submit)
  → YOUR ADAPTER: generate()                          providers/{image,video,model3d}/<p>.rs
  → video and 3D only: poll_status(), from client polls and from the
    background poller (proxy/poller.rs); 3D files are re-hosted in that tick
```

Three facts about this path shape every adapter:

- The validator enforces the YAML, and the adapter trusts it. Anything the
  YAML lets through has to be something the vendor accepts.
- After a retryable error or a timeout, the router calls `generate()` again.
  That is a new vendor job, and the vendor bills it.
- The dashboard has no provider-specific code. It renders credential forms
  from `GET /v1/providers` (`provider_catalog()` in `proxy/registry.rs`) and
  every model control from `GET /v1/models`. Get the registry and the YAML
  right and the dashboard works unchanged. If a provider seems to need a UI
  change, ask the user first.

## Workflow

1. **Classify the change** and open `references/touchpoints.md` at that
   section. The sections are: a new model on an existing adapter, an update
   to a carried model, a retirement or rename, a new family for an existing
   vendor, and a new vendor. Each lists every file to touch.
2. **Read the vendor first-hand.** For every model you touch, find the API
   reference page, the machine-readable request schema and the pricing page.
   Write down each URL with today's date. Prefer an official spec (OpenAPI,
   JSON schema, official SDK source), then the docs site's markdown twin
   (`llms.txt`, `<page>.md`), then rendered HTML. Never take wire facts from
   third-party wrappers or blog posts. When the vendor contradicts itself,
   quote both, take the conservative reading, and say so.
3. **Read the closest adapter** in the same family, and the vendor's other
   adapters if it has any. Copy specific patterns, never a whole file. Every
   existing adapter still has at least one entry in
   `references/known-gaps.md`.
   - From `image/bfl.rs` or `video/vidu.rs`, take the `AuthSpec`, the pool,
     `creds()` and `configure()`.
   - From `model3d/fal.rs`, take the stateless handle, error parsing that
     covers every envelope, merging `extra` without letting it override
     owned keys, trusted-origin poll URLs, and SSRF-checked downloads.

   Check each pattern you copy against the rules below.
4. **Write the failing tests first.** That means the conformance fact for
   each vendor fact the YAML will encode, and the wiremock assertion for each
   outbound field. Watch them fail before you write the code.
5. **Write the YAML rows, then the adapter**, following the rules below.
6. **Wire it up** from `references/touchpoints.md`: registry, config, drift
   definition, docs, landing.
7. **Verify** with `references/testing.md`, and run a live generation when you
   have the credentials and the go-ahead to spend.
8. **Report** in the shape given under [Finish](#finish).

## Rules

Breaking any of these has already shipped a bug here. The reference files
explain each one in more depth.

### Catalog

1. **The catalog is a promise.** Advertise only what the adapter implements
   and what the vendor's schema accepts. That covers model ids, params, enums,
   bounds, prompt limits and ref roles. For image and video, the validator
   doesn't gate on the `capabilities` flags; they only feed `GET /v1/models`.
   (A 3D gate for text-only requests is landing as this is written.) So when
   a model *requires* a mode, such as image-to-video only, express it as a
   ref role with `required: true, min_count: 1`. The adapter also refuses an
   unsupported mode with `ProviderError::InvalidRequest` before it calls the
   vendor. Details are in `references/catalog-yaml.md`.
2. **Cite the vendor where the next person will look.** Citations go in
   three places:
   - `docs_url:` on every new model row. It is the vendor's human-readable
     API page for the exact endpoint that model calls, as an absolute https
     URL, and the loader rejects any other form. `tests/model3d_conformance.rs`
     (`every_vendor_3d_model_names_its_api_reference`) requires it on every
     vendor 3D row. Nothing requires it on image or video rows yet, so assert
     it in your own conformance test.
   - YAML comments. Put `# @see <schema url> (YYYY-MM-DD)` above the typings
     it proves. Put `# @see <pricing url> (YYYY-MM-DD)` beside `pricing:`,
     with the vendor's price string quoted and the arithmetic that gives
     `base_cost_usd`.
   - The adapter's doc header. List the vendor's protocol pages (submit,
     status, result, errors, auth, file inputs), each as `@see <url>` with a
     `Verbatim: "…"` quote of the line that proves the endpoint, header or
     field. Each per-model arm of the resolution table cites that model's API
     page, schema and pricing. Field-mapping comments name the vendor's
     schema type and field.

   Audits re-verify a provider by opening exactly these URLs, and so does
   `/model-drift` when it repairs a source that has moved. The drift check
   itself fetches only its definitions' `sources`. A verbatim quote lets a
   reviewer confirm a mapping without redoing the research, and the date
   shows how old the evidence is.
3. **The price is what the router bills.** The router bills
   `pricing.base_cost_usd`, plus markup, whatever the adapter says. For
   images it multiplies by the number of images delivered. For video and 3D
   it bills once, at submit, and the quota is settled then, so a job that
   later fails is still charged. Set `base_cost_usd` to the vendor's price for the model's
   *default* request (default duration, resolution and size). Show the
   arithmetic in the comment, and record conditional multipliers in
   `variable_pricing` as documentation only. `estimate_cost` returns the same
   flat number. If some allowed params change the vendor price a lot (4K,
   10 s, ×3 for textures), say so in your report. Flat billing under-charges
   those requests, and the fix is the user's call. Details are in
   `references/drift-and-pricing.md`.

### Auth, credentials and URLs

4. **Use one provider name per vendor, across every family.** The registry
   key is also the YAML `provider:`, the model-id prefix, the `providers.<p>`
   config block (and its env vars), the dashboard credential entry and the
   BYO lookup key. A second family
   for an existing vendor reuses the same name, so the customer's existing
   key works for it with no new setup. Never add a name like `<vendor>-video`.
5. **Declare auth instead of hand-writing headers.** Give the adapter an
   `auth: AuthSpec` field set in `new()`. Apply it with
   `crate::providers::auth::apply(&self.auth, &creds, builder)`. For signing
   schemes, call `sigv4::sign` or `tc3::sign` with the final body bytes
   instead. Every adapter for a vendor uses the same scheme and credential
   fields. Only a signing `service` may differ by product, as Tencent's
   `hunyuan`, `aiart` and `vclm` do. A new scheme means
   a new `AuthSpec` variant plus a `provider_catalog()` entry. Details are in
   `references/auth-and-credentials.md`.
6. **Credentials arrive as pools.** The dashboard stores `{"api_keys":[…]}` or
   `{"credential_sets":[…]}` through
   `POST /v1/orgs/{org_id}/provider-credentials`, usually with no single
   `api_key`. In `configure()`, build `ApiKeyPool::shared` or
   `CredentialPool::shared`, but only when the list is non-empty. Pick a
   credential per request. Make `is_configured()` true only when a credential
   can actually be reached. The registry builds a fresh instance for every
   BYO request, from the registered config (or a default one) plus the org's
   credential. That credential may carry an empty pool, and tests build
   instances from a bare default config. So `new()` and `configure()` must be
   cheap and must never panic.
7. **Build every URL from `api_base()`.** One function returns
   `config.api_base` when it is set, and otherwise the vendor's documented
   production base. Build submit, poll, upload and health-check URLs from it.
   Use URLs the vendor hands back (`polling_url`, `status_url`, signed
   downloads) as given. Operators point `api_base` at regional hosts (MiniMax
   `.io` vs `.com`, BytePlus vs Volcengine) and at proxies, and tests point it
   at wiremock. One hard-coded host anywhere bypasses all three. Before you
   attach credentials to a URL taken from a response, check that its host is
   the vendor's API host or the `api_base` host. Otherwise you may send a key
   to any host a response names, and signed download URLs need no key anyway.
   `trusted_queue_url` in `model3d/fal.rs`, with its test
   `the_key_only_goes_to_the_configured_queue_origin`, shows the pattern.
8. **Keep secrets out of logs.** Never put credentials, signed URLs or auth
   headers in `tracing` fields, error messages or `provider_error`. The
   `Debug` output of `ProviderCredentials` prints the secrets.

### Requests

9. **Resolve model ids explicitly, and never guess.** When the catalog suffix
   is the vendor's id, pass it through verbatim (`id.strip_prefix("<p>/")`).
   Otherwise use a table with one arm per catalog row, whose unknown arm
   returns an error. A `_ =>` arm that falls back to a default model bills a
   typo as a different model. Tie the table to the catalog with a test that
   iterates `CapabilityRegistry::for_provider("<p>")`, not a copied list of ids.
10. **Map every declared param to the vendor's own field, per endpoint.**
    Rename fields (`steps` → `num_inference_steps`, `seed` → `random_seed`),
    convert types (`duration` as `"5s"` or as `5`) and reshape values to match
    the schema. Never send a field the endpoint's schema lacks. Never
    silently drop a declared param either: if no endpoint the row reaches
    can take it, the model must not declare it. If only some modes take it
    (`aspect_ratio` on text-to-video but not image-to-video, for example),
    say in the YAML comment which modes ignore it, and test that behavior.
    A derived value (size → ratio, resolution → preset) must come from a
    named table of the vendor's enum, with a test proving the function can't
    return anything else.
11. **Don't let `extra` override fields you own.** `extra_allowlist` is
    enforced only in strict mode. With `strict: false`, `extra` reaches the
    adapter unfiltered. Merge `extra` before you write the fields you own, or
    skip those keys. The fields you own are the model or endpoint id, the
    output count, the output format that sets `content_type`, and the prompt.
    Otherwise a caller's `extra.model` could run a different vendor model from
    the one litegen bills.
12. **Don't forward `n`.** An adapter returns one output, the router bills
    one, and nothing caps `n`. Forwarding it as `n`, `num_images` or
    `candidateCount` makes the vendor generate and bill N. Send the vendor's
    single-output value, or leave the field out.

### Responses, errors and time

13. **Retry only what is safe to repeat.** What `retryable` means depends on
    where the error comes from.
    - In `generate()`, the router calls `generate()` again after a
      retryable error or a 120 s attempt timeout, which submits a new
      vendor job. Mark errors retryable only when you know the vendor didn't
      accept the job: a connect error (nothing was sent), a 429, or a 5xx on
      submit. A timeout or dropped connection *after* the request went out
      is ambiguous, because the vendor may have accepted and billed it. Treat
      it as non-retryable unless the vendor supports idempotency keys.
      Once the vendor has accepted the job, any later error inside
      `generate()` is non-retryable (an image adapter's internal polling,
      its result fetch, its download). Otherwise you pay for the job twice.
    - In `poll_status()` (video and 3D), a **vendor verdict is `Ok`, not
      `Err`**. That covers failed, cancelled, moderated and expired jobs,
      and a job that finished but whose result can no longer be fetched.
      Return them as `Ok(… status: Failed, error: Some(reason) …)`. That takes
      the normal terminal path, which fires the webhook. `Err` means "we
      don't know yet":
      - Make it retryable when asking again can help (5xx, 408, 429,
        connection errors); the poller retries next tick.
      - Make it non-retryable only when the job's state can never be learned
        (a 401 or 404 on the status call, a malformed handle). The poller
        then *reaps* the row as failed **without firing a webhook**.

    Any other 4xx is non-retryable. Note that `ProviderError::Timeout` and
    `RateLimited` are retryable *by type*. So once the vendor has accepted a
    job inside `generate()`, never return them (a poll loop running out of
    attempts included). Return `RequestFailed { retryable: false, .. }`
    instead. For polls, `providers::video::read_poll_json` implements the
    poll-side policy; reuse it. In every case, set `status_code`, put the
    vendor body in `provider_error`, and parse the vendor's message out of
    every error envelope it documents. Never `unwrap` vendor JSON.
14. **Map every documented status.** List the vendor's status enum and map
    each terminal state explicitly. Cancelled, expired, moderated and
    content-filtered all map to `Failed` with the vendor's reason, never to
    `Pending`. Report honest `progress`: the vendor's percentage if it gives
    one, otherwise a fixed mid value, and 100 once terminal.
15. **Keep polling stateless.** `poll_status` receives only its handle, and
    may run on a different instance from the submit, possibly after a
    restart. Video client polls use the platform instance, 3D client polls
    reuse the submitting instance, and the poller builds a fresh BYO
    instance. Put everything polling needs into the handle:
    `provider_job_id` for video, `stage_context` for 3D.
16. **Return output the caller can use.** That means image bytes with an
    accurate `content_type`, a video URL a client can fetch without vendor
    auth, and 3D bytes ready for re-hosting. Download vendor URLs promptly,
    because many expire within minutes. Check the HTTP status and cap the
    download size. A URL that came from a response is untrusted input, so
    pass it through `crate::util::ssrf::validate_public_url` before you
    fetch it server-side, as the fal 3D downloads do.
17. **Fit the time budget.** On direct dispatch, `generate()` gets 120 s per
    attempt. Video and 3D adapters only submit inside `generate()`. Image
    adapters that poll internally must finish well inside the 120 s.
18. **Propagate traces.** Wrap every outbound request builder in
    `crate::providers::inject_trace_headers(...)`.

## Testing and verification

The following is required every time. Templates and commands are in
`references/testing.md`.

- Adapter `#[cfg(test)]` tests with wiremock, run against the real catalog
  row (`ref_schema(id)` loads `models/`). Cover:
  - the method and path, and the auth header from a pooled key;
  - every outbound field name and value, with absent fields asserted
    non-vacuously;
  - ref encoding, response parsing, and every error envelope;
  - for async families, every status mapping.
- A test that iterates the catalog, tying the adapter's resolution table to
  the YAML rows.
- Conformance facts in `litegen-core/tests/catalog_conformance.rs`. 3D facts
  go in its "3D family conformance" section, or in a dedicated
  `model3d_conformance.rs` if your checkout has one. Write one test per
  vendor fact the YAML encodes, each citing `@see <url> (date)`, and include
  `docs_url`.
- A drift definition that maps every model, with its tests passing.
- A live run: one real generation per family and request shape you added,
  when credentials exist and the user is fine with the spend. Inspect the
  output: decode the image, fetch the video URL, check the mesh structure. If
  you didn't run it, write "not verified live" in the report, plainly.
- An update to `docs/superpowers/param-mapping-matrix.md` for every param ×
  provider cell you touched. Tick a cell ✅ only when a named test proves it.

## Model drift: keep changes detectable

`/model-drift` diffs every vendor against the catalog each week. It only
catches what you hook up. Details are in `references/drift-and-pricing.md`.

- Map every catalog id to every vendor id the adapter sends, in
  `scripts/model-drift/providers/<p>.mjs`. Use an array when the adapter
  routes a model to several endpoints. The definitions test fails on an
  unmapped id, and that is exactly how it went red when the fal 3D models
  shipped without mappings.
- Give every carried id typings coverage: a source whose `snapshot` holds the
  request schema of the endpoints the adapter calls. Where the same page
  publishes prices, keep the price lines in the snapshot so a price change
  shows up as `changed`. Replicate's definition does this.
- For an announced shutdown, add the model to `SCHEDULED_SHUTDOWNS` in
  `catalog_conformance.rs` and prefix its description with `DEPRECATED:` and
  the date. Vendor models you skip go in `acknowledged`, with a dated reason.
- Verify a definition by running the check twice against the *same* temp
  directory: `D=$(mktemp -d)`, then
  `node scripts/model-drift/check.mjs --provider <p> --snapshots "$D"` twice.
  The second run must show nothing `changed`. Two fresh directories can
  never show `changed`, so they prove nothing. Leave `snapshots/` alone: the
  weekly run commits reviewed snapshots.
- Write each id in the definition as a quoted string literal. A Rust test
  checks that every catalog id is mapped by reading the `.mjs` text, so an id
  built with a template string fails it.

## Decisions that belong to the user

Bring these to the user instead of choosing:

- Which vendor models to carry or skip, and what replaces a retiring model.
- Prices: changing `base_cost_usd` on a carried model, and price spreads that
  flat billing can't capture.
- Spending money on live runs, unless the user already asked for live
  verification.
- Core changes: a new canonical param or `ParamSpec` kind, router, poller,
  storage or dashboard changes, and fixes to the known gaps.
- Any vendor rule that forces you to under-advertise. Record it under
  "Can't express" in the report.

## Finish

1. Run everything under "Before you report" in `references/testing.md`.
2. Run the consistency sweep in `references/touchpoints.md` for every id you
   added, renamed or removed. Run the grep untruncated: piping it through
   `| head` once hid three failing tests and the public site.
3. Report in this shape:
   1. **Changed:** files and models, one line each.
   2. **Verified:** test names, plus any live runs with their cost and what
      you checked in the output.
   3. **Not verified:** anything covered only by mocks, said plainly.
   4. **Decide:** product and pricing questions, with vendor URLs.
   5. **Can't express:** vendor rules the schema can't represent, and what we
      accept instead.
   6. **Gaps hit:** any entry from `references/known-gaps.md` this work ran
      into.
4. Commit only when the user asks. Other sessions share this working tree, so
   stage by explicit path: `git add <paths> && git commit -m "…" -- <paths>`.
   Follow the repo's commit style (`feat(providers): …`, `feat(catalog): …`,
   `fix(<provider>): …`, `test(catalog): …`), with a body that says why and
   what was verified.

## Reference files

- `references/touchpoints.md`: every file to touch for each type of change,
  the consistency sweep, and downstream surfaces (landing, README, SDKs,
  price-api).
- `references/auth-and-credentials.md`: the schemes, the adapter credential
  pattern, how a dashboard key reaches the adapter, and the tests that prove
  "it just works".
- `references/catalog-yaml.md`: the row fields, canonical param kinds, ref
  roles and forms, capabilities vs required roles, strict and lax mode, and
  `docs_url`.
- `references/testing.md`: test layers, helper idioms, anti-vacuous rules,
  commands, and the bar for live verification.
- `references/drift-and-pricing.md`: authoring a drift definition, the
  shutdown lifecycle, and pricing evidence the weekly run can check.
- `references/known-gaps.md`: core gaps and legacy patterns, as of
  2026-10-12, that an adapter must work around or report, but not paper over.
