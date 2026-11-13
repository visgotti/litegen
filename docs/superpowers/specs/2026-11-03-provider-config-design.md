# Provider config that works everywhere — design

Round 3 of the known-gaps work (the user, 2026-11-03: "fix the gaps to ensure
our app has full flexibility so users can use it however they need to"). The
user delegated the design ("research best practices … build it in the best
way u can"), so every decision below is mine unless it quotes them.

## Why

An operator configures a provider in `litegen.yaml` (or `LITEGEN__PROVIDERS__…`)
with `api_key(s)`, `api_base`, `model_mapping`, `extra_headers` and `options`.
Today only some adapters honor those fields, so the same config means
different things per vendor:

- no adapter sends `extra_headers`, so routing through an AI gateway that needs
  its own header (Cloudflare AI Gateway's `cf-aig-authorization`, Helicone's
  `Helicone-Auth`, an `OpenAI-Organization` header) is impossible;
- `model_mapping` works in six adapters out of about forty;
- ten adapters write their auth headers by hand, so they can drift from the
  scheme the dashboard shows;
- several adapters retry a 429 without the vendor's `Retry-After`, treat an
  unknown vendor status as "still running" for two hours, fail an accepted
  (billed) job on one unreadable poll, or let a passthrough `extra` key
  overwrite a field litegen sets;
- a `values` list on a number param loads but isn't checked;
- four video rows take the start frame only as `init`, so the API reports
  `supports_first_frame: false` for them;
- fal's health check sends the key to fal's own host even when `api_base`
  routes fal through a proxy.

## Goals

1. **`extra_headers`** reach every request litegen makes to a vendor's API
   (submit, poll, cancel, health check), and never a result download from a
   CDN or object store.
2. **`model_mapping`** changes the model id the vendor sees, in every adapter
   whose vendor takes one.
3. **One auth path:** every adapter declares an `AuthSpec` and applies it with
   `auth::apply` (or its signer).
4. **Retries see the vendor's wait:** every submit reads a 429 through
   `read_vendor_json`, so `Retry-After` sets the floor of the backoff.
5. **An undocumented vendor status fails the job at once** and names the
   status, in every adapter that polls (video, image, 3D).
6. **A poll never fails an accepted job for a transient reason** (Luma image).
7. **Passthrough `extra` never overwrites a field litegen sets** (Hunyuan
   image and video, Luma image).
8. **`values` on any int or float param is enforced**, on first-class params
   and on typed `extra_allowlist` params alike.
9. **The four `init` start-frame rows take `first_frame`** as well.
10. **No key goes to a host outside `api_base`** unless the operator names it.

## Non-goals

- Per-tenant `api_base`, headers or mappings on BYO credentials. A tenant
  that can point litegen at any host is an SSRF hole; that needs its own
  design (an allowlist the operator controls). BYO credentials stay
  credentials only.
- Live vendor runs: this session has no vendor keys. Behavior is proven with
  mock vendors; the report says so.
- New catalog rows or prices (round 4 and 5).

## Decisions

### extra_headers

- Each adapter's **API client** is built with the operator's headers as
  default headers (`providers::api_client(builder, &extra_headers)`), when
  the adapter is configured. A header the request sets itself (auth,
  `content-type`) wins over a default of the same name, so `extra_headers`
  can add but never replace auth.
- **Downloads never carry them.** Result downloads already use a separate
  no-redirect `asset_client` in most adapters; Hunyuan, Kling, Luma and
  ByteDance image download with their API client today, so they move to
  `image::fetch_result` (the same public-address check and 64 MiB cap the
  others have).
- **Config validation** (startup and the config reload) refuses a header
  name or value that isn't valid HTTP, and the framing and hop-by-hop
  headers a default would break: `host`, `content-length`,
  `transfer-encoding`, `connection`, `keep-alive`, `upgrade`, `te`,
  `trailer`, `proxy-authorization`, `proxy-connection`. The error names the
  provider and the header.
- Signing schemes (Bedrock SigV4, Hunyuan TC3) sign their own headers;
  extra headers go out unsigned, which both schemes allow. Documented.

### model_mapping

- `ProviderInstanceConfig::mapped_model(model_id) -> Option<&str>` looks the
  id up by the full catalog id (`bfl/flux-pro-1.1`), then by the id without
  its provider prefix (`flux-pro-1.1`).
- The mapped value replaces only the string the vendor receives (a body
  field or the URL path segment that names the model). The catalog row still
  decides params, price, request shape and endpoint choice.
- Vendors with no model id on the wire (one endpoint per model, chosen by
  litegen, with no free-form id) can't be mapped; the docs list them, and
  a startup warning names any mapping key that matches no catalog model of
  that provider, or a provider that takes no model id.

### Auth

- openai (image, video), stability, replicate (image, video), luma video and
  runway video declare `AuthSpec::bearer()`; fal (image, video, 3D) declares
  `Authorization: Key <key>` as an `AuthSpec::Header`. All call
  `auth::apply` with the per-request credentials (key pool pick included).

### 429s and unknown statuses

- Luma video, PixelLab, Google video, OpenAI video and Bedrock image and
  video read submit answers with `read_vendor_json`.
- Every polling adapter maps each state its vendor documents (cited with
  `@see`); any other state, or a missing one on a 2xx answer, is `Failed`
  with `"<vendor> reported status <s>"`, as Replicate does today. In-progress
  mappings don't change.

### Luma image poll

- The poll reads answers with `read_poll_json`: a transport error, 5xx, 408
  or 429 asks again within the poll budget; any other error status fails
  the call; a `failed` state fails it with Luma's reason.

### Numeric value sets

- The validator checks `values` on every int and float param it validates
  (`fps`, `steps`, `n`, `guidance_scale`, `strength`, `target_polycount`
  and the rest) and on typed `extra_allowlist` params: 400
  `param_enum_mismatch` naming the allowed values.
- The loader refuses a `values` entry outside the param's `[min, max]`.
- Rows that wrote "the adapter rejects …" in a comment state the set:
  `fal/ltx-2.3` fps `[24, 25, 48, 50]`, `hunyuan` image-to-video fps
  `[16, 24, 30]`. The adapters keep their own checks as a second line.

### Start frames

- `fal/video`, `fal/ltx-2.3`, `runway/gen4-turbo` and `runway/gen4.5` add a
  `first_frame` role and make it the default role; `init` stays accepted for
  existing callers. Each adapter reads `first_frame`, else `init`.
  `max_total: 1` still refuses both at once.

### fal's Platform API

- With a custom `api_base` and no `options.platform_api_base`, fal's health
  check doesn't call `api.fal.ai`; it answers healthy with "reached through
  {api_base}; fal's Platform API isn't checked (set
  options.platform_api_base)". A default config keeps today's check.

## Testing

- A crate-level conformance test builds every adapter that serves a catalog
  row, pointed at one mock vendor, with an extra header and a mapping for
  one of its models. It drives one generation per adapter and family, and
  asserts that every request the mock saw carries the extra header and the
  adapter's auth, and that one carries the mapped id (or that the adapter is
  on the documented no-model-id list).
- Unit and wiremock tests for each other decision, each watched failing
  first.
- The full cargo gate with Postgres, the dashboard and SDK suites (the
  catalog changes reach `/v1/models`).

## Left to the builder

- Per-adapter details: which body field or path segment carries the model;
  which documented states each vendor has (from the `@see` already in each
  adapter, re-read where missing).
