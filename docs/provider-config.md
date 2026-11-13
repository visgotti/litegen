# Provider config: keys, hosts, headers, model names

Every vendor litegen calls is configured the same way, in `litegen.yaml`
(`providers.<name>`), in `LITEGEN__PROVIDERS__<NAME>__<FIELD>` environment
variables, or at runtime through `PUT /v1/admin/providers/{name}`
([management-api.md](management-api.md)). A field set at runtime wins over
the file; a field left unset falls back to it. In-flight jobs keep the
config they started with.

At startup, for each field: a `LITEGEN__PROVIDERS__<NAME>__<FIELD>` variable
beats the vendor's own variable (`OPENAI_API_KEY`, `FAL_KEY`, …), which beats
the file. A blank vendor variable counts as unset, so compose's
`${OPENAI_API_KEY:-}` never blanks a key set elsewhere, and a startup warning
names a variable or file value that another one hides. In the file, a key can
also come from a variable by name, LiteLLM's way: `api_key: os.environ/OPENAI_API_KEY`.
The whole order is in [configuration.md](configuration.md#which-source-wins).

```yaml
providers:
  openai:
    api_key: sk-...                  # or api_keys: "sk-a:3,sk-b:1" (weighted)
    api_base: https://gateway.example/openai/v1
    extra_headers:
      cf-aig-authorization: Bearer cf-token
      OpenAI-Organization: org-123
    model_mapping:
      openai/gpt-image-2: my-gpt-image-deployment
  fal:
    api_key: ...
    api_base: https://gateway.example/fal
    options:
      platform_api_base: https://gateway.example/fal-platform
```

These fields apply to every vendor (a test drives every adapter against a
mock vendor to hold them to it: `proxy/config_conformance.rs`).

## `api_key`, `api_keys`

One key, or a weighted pool (`"key:weight,key"`), rotated per request.
Signing vendors (Bedrock, Hunyuan) take `key_id`, `key_secret` and
`region` instead. Kling takes an API key since 2026-11-03 (its access/secret
key pair works only on its legacy API design). An organization's own key (BYO, set on the dashboard's
Providers page) replaces these per request; everything else on this page
still comes from the operator's config.

## `api_base`

Where litegen sends the vendor's API calls: the vendor's own host by
default, or a proxy or AI gateway (Cloudflare AI Gateway, Helicone, a
corporate egress proxy). Every API call goes there: submits, polls,
cancels and health checks. The key is never sent to a host the operator
didn't configure:

- **fal** takes one `api_base` for its queue (video and 3D) and its sync
  host (images): fal's own queue host means fal's sync host for images, and
  any other base is used for both. Its health check reads fal's Platform
  API, which a proxy may not serve: set `options.platform_api_base` to have
  it checked; without it, through a proxy, the health check reports the
  key unchecked instead of calling `api.fal.ai`. `options.sync_api_base`
  names the image host outright.
- **BFL** answers each job with a `polling_url` on the regional host that
  holds the task. litegen follows it when it is on `api_base`'s origin or an
  https `*.bfl.ai` host, and refuses any other.

Result files (images, videos, meshes) are downloaded from wherever the
vendor stores them, without the key.

## `extra_headers`

Headers added to every request litegen sends to the vendor's API: a gateway's
own auth (`cf-aig-authorization`, `Helicone-Auth`), an
`OpenAI-Organization` or `OpenAI-Project`, a tracing or tenant header.

- A header litegen sets itself on a request (the vendor's auth,
  `Content-Type`) wins over an extra header of the same name: extra headers
  add, and never replace the vendor's auth.
- They never go to result downloads or to a vendor's presigned upload URL
  (object stores and CDNs aren't the vendor's API).
- On signing vendors (Bedrock's SigV4, Tencent's TC3) they go out unsigned,
  which both schemes allow.
- litegen refuses, at startup and from the admin API, a header that isn't
  valid HTTP and the framing and hop-by-hop headers a default would break:
  `Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, `Keep-Alive`,
  `Upgrade`, `TE`, `Trailer`, `Proxy-Authorization`, `Proxy-Connection`. The
  error names the provider and the header.

## `model_mapping`

Sends a catalog model to the vendor under another name: a vendor's newer
version before litegen's catalog carries it, an Azure-style deployment
name, a fine-tune, a private fal app.

- Keys are catalog ids (`bfl/flux-pro-1.1`) or ids without the provider
  prefix (`flux-pro-1.1`); the full id wins when both are set.
- Only the name the vendor sees changes: the body field or URL path segment
  that names the model. The catalog row still decides the params litegen
  accepts, the price it bills and the endpoint it calls. On fal, where the
  endpoint path is the model, the mapping replaces the path in whichever
  mode the request's references chose; map a row with a text and an image
  endpoint only to an app that serves both.
- Replicate takes `owner/name:version`, or a bare version hash.
- Some vendors take no model id at all: litegen picks the endpoint or action
  per row. A mapping for them does nothing: Ideogram (endpoint and
  `rendering_speed`), PixelLab (one endpoint per model), Hunyuan (Tencent's
  `X-TC-Action`) and Recraft's background removal.
- At startup litegen logs a warning for each mapping key that names no model
  of its provider (a typo, a retired id) or that can't be used.

## `options`

Vendor-specific settings, read only by the adapter they belong to:

| Provider | Option | What it does |
|---|---|---|
| fal | `sync_api_base` | The host images run on (default: fal's sync host, or `api_base` when that is a proxy). |
| fal | `platform_api_base` | fal's Platform API, which the health check reads. |
| bedrock (video) | `s3_output_uri` | The S3 prefix Nova Reel writes its videos to. |

## What litegen does with a vendor's answers

These hold for every vendor:

- A rate limit (`429`) is retried after the wait the vendor names
  (`Retry-After`), with backoff and jitter
  ([gateway-settings.md](gateway-settings.md)).
- A gateway's `5xx` page, a `408` or a `429` on a submit is retried; on a
  poll it is asked again. A job the vendor accepted is never submitted twice.
- A job state the vendor doesn't document fails the generation at once,
  naming the state, rather than polling until the job's maximum age.
- A request's `extra` passes vendor knobs through, never over a field
  litegen sets (the model, the prompt, the size, the references). It never
  sets where a result goes or who can see it either: a vendor's callback or
  webhook URL, a storage destination or a publish switch (`isPublic`,
  `public`). litegen polls every job and fires your key's webhook, handing
  files out in your app's access mode ([storage-access.md](storage-access.md)).
- A key the model types in `extra_params` (`GET /v1/models/{id}`) is checked
  like a param, in either mode: a wrong value is 400 with
  `param: "extra.<key>"` before any vendor call.
- A number param whose catalog row lists `values` takes only those
  (400 `param_enum_mismatch`), and the dashboard offers them as a list.

## Upgrading a custom model folder or runtime model

A deployment with its own `LITEGEN_MODELS_DIR`, or with runtime models
(`POST /v1/admin/models`), may need these edits. A file that breaks a rule
stops the server at boot, naming the model and the key. A stored runtime
model that breaks one is skipped at boot and on refresh, with the reason in
the log.

- **Fields litegen owns can't be allowlisted.** `extra_allowlist` may not
  name a field that says where a result goes, where it is stored, or whether
  the vendor publishes it. That covers `callback_url`, `webhook_url`,
  `notify_url`, `storage_uri`, `output_data_config`, `is_public` and
  `public`, and any key with `callback` or `webhook` in it. Case, `_` and `-`
  are ignored, so `isPublic` and `CallbackUrl` count too. Remove them. The
  shipped rows dropped `callback_url` (Vidu, MiniMax, BytePlus, Luma) and
  `isPublic`, `imageId` and `imageType` (Leonardo video).
- **Unknown keys are refused.** Every catalog object rejects a key it doesn't
  name, at any depth, so a typo (`max_lenght`, `extra_alowlist`) no longer
  loads silently. Watch for a description in a `{ … }` flow mapping that
  holds a comma: YAML ends a plain value at `, `, so quote the text
  (`description: "The rectangle to edit, sent drawn as the mask."`). A
  `provider_format` of `url` or `base64` takes no other key. `status:` is
  accepted and ignored: litegen derives it from the dates.
- **Type an extra a price reads.** A `variable_pricing` rule whose `when`
  reads `extra.<key>` needs that key typed in `extra_params`, and its value
  must be one the type takes (`"*"` for any value, or a range for a number).
  A price compares the value exactly, so `"true"` would never match `true`.
  The shipped rows type `fal/hyper3d-rodin`'s `addons` and Runway's
  `preciseEdit` and `draft`.
- **`extra_params`** is optional for every other key. It types an
  allowlisted key with `kind: bool`, `int`, `float`, `string` or
  `string_array`, plus the usual bounds (`min`, `max`, `values`,
  `enum_values`, `max_length`, `max_items`, `pattern`, `default`). A key it
  doesn't type passes as any JSON, as before.
- **A `pattern` is enforced.** A string param's or typed extra's `pattern` is
  a regular expression, checked at load and matched anywhere in the value, as
  JSON Schema does. Anchor it with `^…$` to match the whole value.
