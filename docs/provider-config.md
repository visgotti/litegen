# Provider config: keys, hosts, headers, model names

Every vendor litegen calls is configured the same way, in `litegen.yaml`
(`providers.<name>`), in `LITEGEN__PROVIDERS__<NAME>__<FIELD>` environment
variables, or at runtime through `PUT` or `PATCH /v1/admin/providers/{name}`
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
still comes from the operator's config. A key saved through the admin API
stays until a `PATCH` sends it as `""` (the file's key applies again) or the
runtime config is deleted; a `PUT` or `PATCH` that leaves it out keeps it.

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

The user info of an `api_base` (`https://user:pass@proxy.example/v1`, a
proxy's basic auth) is a credential. litegen calls the whole URL, but the
provider view (`GET /v1/admin/providers`) and the audit log show
`https://[redacted]@proxy.example/v1`. A `PUT` or `PATCH` that sends that
value back unchanged, as the dashboard does with every save, keeps the URL
in force. To change the URL, send it whole, user info included: any other
URL with `[redacted]` for its user info is refused (400
`invalid_provider_config`). A URL in `options` is shown, kept and refused
the same way. The user info is stored in plain text unless
`encrypt_provider_credentials` is on ([Credentials at rest](#credentials-at-rest)).

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
- The admin API never shows their values. The provider view lists their
  names (`extra_header_names`), and the audit log keeps the names, never the
  values: a change's `provider.configure` entry shows `[redacted]` for each
  one. Entries written before 2026-11-14 kept the values. Migration 027
  redacts them, and the runbook says which tokens to rotate
  ([RUNBOOK.md](RUNBOOK.md#provider-header-tokens-in-old-audit-entries)).
- A value set through the admin API is stored in plain text in
  `provider_configs.settings_json` while the gateway setting
  `encrypt_provider_credentials` is off, the default, and encrypted with the
  provider's keys once it is on ([Credentials at rest](#credentials-at-rest)).
  Headers set in `litegen.yaml` or in variables aren't stored in the
  database.
- A `PUT` replaces the settings whole: `extra_headers` replaces the stored
  map, and a `PUT` without it drops the runtime headers (the file's apply
  again). Only the keys, left out, keep their stored value.
- A `PATCH` changes only the fields it sends, so a save of a few fields, as
  the dashboard's Configure form makes, keeps the headers. `extra_headers`
  sent replaces the stored map whole, `null` drops it (the file's apply
  again), and a `PATCH` without it keeps it. A key left out or `null` keeps
  its stored value; one sent as `""` removes it, and the file's key applies
  again.

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
| bedrock (video) | `s3_api_base` | The S3 host litegen reads the finished videos from (default: the bucket's own AWS host): an S3-compatible proxy. |

The provider view and the audit log show each URL in `options` with
`[redacted]` for its user info, as for `api_base`, and a save that sends it
back as shown keeps the URL in force.

## Credentials at rest

A runtime provider config (one saved through the admin API or the
dashboard) is stored in `provider_configs`. Its keys (`api_key`,
`api_keys`, `key_id`, `key_secret`) are always encrypted with
`LITEGEN__SECRETS_KEY` (AES-256-GCM); without the key, a key can't be saved.
Its other credentials, each `extra_headers` value and the user info of
`api_base` and of the URLs in `options`, follow the gateway setting
`encrypt_provider_credentials` (`PUT /v1/admin/settings`):

- **Off**, the default: they are stored in plain text in `settings_json`,
  and the row is exactly what versions before 2026-11-15 write and read.
- **On** (it needs `LITEGEN__SECRETS_KEY`, else the save is a 400
  `secrets_key_missing`): they are encrypted with the keys, and
  `settings_json` keeps `[redacted]` in their place. A header's name stays
  readable. A version before 2026-11-15 can't read such a row: it would send
  `[redacted]` to the vendor or gateway.

Each save of the gateway settings stores every runtime provider config again
as the setting says, on the replica that takes the save. Turning it on
encrypts what was stored in plain text. Turning it off (a save that sets it
to `false` or `null`) stores each row again as older versions read it.
Unlike the other gateway settings, a save that leaves it out keeps it as
stored, so a client written before it existed can't turn it off. A
row keeps its `updated_at`. When the setting switches, or a row is stored
again, the save logs one line and writes one audit entry,
`provider.encrypt_credentials` or `provider.decrypt_credentials`, with the
number of configs stored again and their names. A save reads the setting
again after its pass, and passes again if another save switched it
meanwhile. A provider save on another replica reads the setting again after
its own write. So a save that races the switch still ends as the setting
that stands says.

A start never stores a row again; it follows the stored setting, and says
what to do, never naming a value:

- With the key set and the setting off: a notice to turn it on once every
  replica runs this version (a single replica can turn it on at once), as
  a warning that names the configs keeping credentials in plain text, if any.
- Configs that keep encrypted credentials while the setting is off: a
  warning naming them, since an older version can't read them. A save of the
  gateway settings stores them as older versions read them.
- Configs that keep credentials in plain text while the setting is on (saved
  by an older version, or by a replica without the key): a warning naming
  them. A save of the gateway settings encrypts them.
- Without the key: a warning naming the configs that keep credentials in
  plain text, and one more if the setting is on, since that replica can't
  read encrypted credentials.

Turning it on, and back off before a rollback, are steps in the runbook
([RUNBOOK.md](RUNBOOK.md#provider-credentials-at-rest)).

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
- **A Leonardo video row can't allowlist `imageId` or `imageType`.** Leonardo
  video starts from the request's own image, which litegen uploads; an image
  id passed in `extra` would animate any image on the gateway's Leonardo
  account, whoever asked. Case, `_` and `-` are ignored here too
  (`image_id`). Remove them; send the start frame as a `first_frame` image.
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
