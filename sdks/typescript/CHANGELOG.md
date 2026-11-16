# Changelog

Every release of `@litegen/sdk`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor release may change behaviour, and says how here.

## [Unreleased]

### Added

- `auth.me()` returns `MeResponse` (it returned `unknown`), and the types
  `MeResponse`, `MeOrg` and `MeUser`: the signed-in person, each organization
  they belong to with their `role` and the `permissions` it holds there, as
  the server checks them (the deployment's own, such as `system:config`, only
  in a single-tenant deployment's default organization), and the
  `active_org` and `active_app` (`string | null`) the request acts in.
- `needs_first_frame` on `RefRoleSpec`: on a video model's `last_frame` role,
  a last frame sent without a `first_frame` (or `init`) image is refused with
  `ref_role_required` (a `LiteGenAPIError` with `param: "reference_images"`)
  before anything is quoted or reserved.
- The video generation route's 502 is in the generated types: `vendor_error`
  (the vendor failed; `providerError` and `retryable` say more) and, on a
  model route, `provider_error` (no deployment was called; `retryable`).
- `package: "zip"` on `models3d.convert`: the answer is a zip
  (`application/zip`, `model.<to>.zip`) holding the converted file first and
  whatever goes beside it, such as an OBJ's `.mtl` and textures. The SDK hands
  the zip back unopened.
- `ktx2Textures` on `models3d.convert`: `"png"` (the default) re-encodes every
  KTX2 (Basis Universal) texture as PNG, and `"keep"` leaves it as it is when
  `to` is `glb` or `gltf`, which then require `KHR_texture_basisu`.
- `ktx2_textures` on 3D generation requests, and its type
  `Model3dKtx2Textures`: the same choice for delivery. `"keep"` with
  `keepOutputs: false` is refused (`option_needs_keeping`).
- `fbxLightUnits` and `usdLightUnits` on `models3d.convert`: the units FBX
  and USD light strengths are stored in at that end, `"photometric"` (the
  default; glTF's candela and lux) or `"blender"` (so the lights open in
  Blender as bright as the glTF does).
- `fbx_light_units` and `usd_light_units` on 3D generation requests, and
  their type `Model3dLightUnits`: the same choice, for when litegen reads the
  vendor's FBX or USD or writes one for the request.
- `daeMorphs` on `models3d.convert`, when `to` is `dae`: `"bake"` (the
  default) writes each mesh at its weights and drops the morph targets'
  animation, which every Collada reader opens; `"keep"` writes the targets as
  `<morph>` controllers with their weight animation, which SceneKit (Xcode)
  and Godot read and three.js and assimp refuse.
- `dae_morphs` on 3D generation requests, and its type `Model3dDaeMorphs`:
  the same choice for a Collada litegen writes for the request. `"keep"` with
  `keepOutputs: false` is refused (`option_needs_keeping`).
- `"usdc"` in `Model3dFormat`: a USD layer written as a binary crate, for
  `models3d.convert`'s `from` and `to` and for `output_formats`. `"usda"` and
  `"usdc"` each read either encoding.
- `usdzRoot` and `usdClips` on `models3d.convert`. `usdzRoot` (when `to` is
  `usdz`) writes the package's root layer as `"usda"` text (the default) or a
  `"usdc"` crate, as Apple's own packagers do. `usdClips` (when `to` is
  `usda`, `usdc` or `usdz`) keeps `"all"` clips as the variant set
  `Animation` or the `"first"` alone; by default all for usda and usdc, the
  first for usdz.
- `usdz_root` and `usd_clips` on 3D generation requests, and their types
  `Model3dUsdzRoot` and `Model3dUsdClips`: the same choices for a USD litegen
  writes for the request. `"usdc"` and `"all"` with `keepOutputs: false` are
  refused (`option_needs_keeping`).
- `output_packages` on 3D generation requests, and the `"package"` asset kind
  it delivers: one zip per requested container that has files to go beside
  its mesh.
- `FramesSetShape`, the type of `frames_set_shape` on a video model's
  `params.aspect_ratio`: whether a `first_frame`, `last_frame` or `init` image
  sets the clip's shape, so `aspect_ratio` is refused (or dropped in lax mode)
  beside one.
- On `ModelSchema` (`models.get`): `litegen_params` (read-only), the fields
  litegen applies to the model itself (`ground`, `center` and `height_m` on 3D
  models; `mask_grow_px`, `preserve_unmasked` and `feather_px` on mask and
  region editors) in the `ParamSpec` shape; `extra_params`, the types of its
  `extra_allowlist` keys; and `status`. A served schema may be posted back to
  `admin.models.create` or `replace` as it came.
- `ModelCapabilities.max_request_bytes` (the largest generation or quote body,
  25 MiB; on every listed model, so typed `number`, not optional),
  `PromptSpec.required_for_text_to_video`, `ProviderHealth.modality`,
  `GatewaySettings.cache_max_bytes` and the cache's `max_bytes` in force, and
  `strict` on `images.segment` requests.
- `admin.providers.patch(name, changes)` (`PATCH /v1/admin/providers/{name}`)
  and its body type `PatchProviderConfigRequest`, resolving to the
  `ProviderConfigView`: it changes some of a provider's runtime config and
  keeps the rest. A field sent is set, `null` clears it (it falls back to
  env/YAML) and a field left out keeps its stored value. A key left out or
  `null` keeps its stored value, and one sent as `""` removes it, so the
  env/YAML key applies again; a `model_mapping`, `extra_headers` or
  `options` sent replaces the stored one whole. `configure` (`PUT`) still
  replaces the whole config.
- `orgs.invitations.list(orgId)` (`GET /v1/orgs/{id}/invitations`): the
  org's invitations not yet accepted or expired, newest first, as
  `PendingInvitation[]` without their tokens; and
  `orgs.invitations.revoke(orgId, invitationId)`, resolving to `void` (404
  `invitation_not_found` when no invitation with that id waits in the org).
  To send a lost link again, revoke the invitation and invite anew. Both,
  like `orgs.members.invite`, take a signed-in session or the master key; an
  API key gets a 403 `forbidden`.
- `orgs.members.invite` resolves to the exported `InviteResponse` (it
  resolved to `unknown`), as `users.invite` does, and `InviteResponse` gains
  `org_id`, `token` and `invite_url` (`string | null`). The token and link
  are shown only in this answer, and litegen sends no email, so hand one of
  them to the invitee; `invite_url` is `null` when the server has no
  `server.public_base_url` (build `/invite/{token}` on the dashboard's
  origin). A 400 is `cannot_invite_owner` or `invalid_email`.
- `expires_at` (`string | null`) and `is_active` on `PatchKeyBody`, the body
  of `keys.patch`.
- `delivery_id` (`string | null`) on `WebhookDelivery`
  (`keys.listWebhookDeliveries`): `whd-` and a UUID, the
  `X-Litegen-Delivery-Id` its receiver got, the same on every attempt of one
  delivery (`null` on attempts recorded before migration 029). Every attempt
  of one delivery shows the payload of its latest attempt.
- `keys.redeliverWebhookDelivery(id, deliveryId)`
  (`POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver`): sends
  a delivery that ended (`delivered` or `failed`) again, as a fresh series
  of attempts under the same `delivery_id`, to the key's `webhook_url` of
  that moment, and resolves to its `WebhookDeliveryState` (`status` a
  `WebhookDeliveryStatus`: `pending`, `delivered` or `failed`), both types
  exported. It throws a 409 `delivery_pending` while the delivery is still
  being sent, a 429 `redeliver_too_soon` within a minute of its last
  redeliver (`retryAfterMs` says when to ask again), a 400
  `no_webhook_configured` or `invalid_webhook_url`, and a 404 for an
  unknown key or delivery.
- `LiteGenAPIError.body`: the error response's body as JSON, when it was
  JSON (optional).
- `users.passwordResetLink(id)` (`POST /v1/users/{id}/password-reset`),
  resolving to the exported `PasswordResetLink`. Its `token` and `reset_url`
  (`null` when the server has no `server.public_base_url`) are shown only in
  this answer and set the account's password once, until `expires_at` (an
  hour); litegen sends no email. The master key may, and so may a signed-in
  person who outranks the account in every org it belongs to, never for their
  own account; for a deactivated account only someone who may reactivate it,
  since confirming the link reactivates it. An API key gets a 403. A 403 is
  `forbidden_permission`, a 404 `user_not_found`.
- `org_id` on `AuditLogEntry`, absent from an entry filed under no org (in
  hosted mode, the master key's changes to the gateway and to accounts, and
  an account change by a person in no org), which only the master key reads;
  and `org_id` in the options of `audit.list` and `audit.exportCsv`: the
  master key reads another org's entries, and anyone else may name only
  their own (else a 403).
- In the generated types: the redeliver route and `WebhookDeliveryState`;
  `cache_hit` on `RequestLog` and `SpendLogEntry` (whether the cache
  answered; a hit has a row of its own, at what it was charged),
  `GatewaySettings.cache_charge_hits` and `EffectiveCache.charge_hits`
  (whether a hit is charged the cached answer's price: by default it is,
  `false` makes hits free); `AuthConfigResponse.signup_open` (whether
  self-service signup is open: always in hosted mode, in single-tenant mode
  while no account exists) and `AuthConfigResponse.password_min_length` (the
  shortest password signup, invitations, password changes and resets
  accept), both required; `RefRoleSpec.not_with` (see Changed);
  `ParamSpecStringArray.returned_together`, on `output_formats`: the
  containers one vendor job returns together (Meshy's rig returns a glb and
  an fbx at once); `GatewaySettings.encrypt_provider_credentials`, which also
  encrypts each `extra_headers` value and the user info of `api_base` and of
  the URLs in `options` at rest (off by default; turning it on needs
  `LITEGEN__SECRETS_KEY`, else a 400 `secrets_key_missing`; unlike the other
  settings, a PUT that leaves it out keeps its stored value);
  `GatewaySettingsView.limits` and the exported `GatewaySettingLimits` and
  `SettingRange` (`min` and `max`, both allowed, for each numeric setting,
  as `PUT /v1/admin/settings` checks them); the 503 `HealthResponse` of
  `GET /health` (`status` `"degraded"`: a provider answered unhealthy); and
  the file routes' 410 (`files_deleted`, `vendor_file_gone`), 500, 502
  (`vendor_file_unreachable`) and 503 (`hook_failed`) answers, the signed
  route's 307.

### Changed

- `AnimationRequest.name` is optional and may be `null`: litegen names a clip
  that leaves it out (`walk` after its preset, else `clip-<n>`).
- `health.ready()` resolves to the server's `ReadinessResponse` (`status`, and
  `checks` with `db` and the providers set up with the gateway's own keys)
  instead of `{ status }` alone.
- What `X-Litegen-Dropped-Params` covers: fields the request has no such name
  for too, one inside an object by its path, on the quotes, `images.segment`
  and the image 202 as well. It lists at most 32 names and ends with a
  `+N more` count past that; `parseDroppedParams`, `droppedParams` and
  `readResponseMeta` keep that entry.
- `LiteGenAPIError.retryable` comes with every router error, such as a 503
  `circuit_open` (every deployment of a model route cooling down, with
  `retryAfterMs`), not only a vendor's. New codes callers may see:
  `circuit_open`, `cache_clear_incomplete` (`cache.clear`, 503),
  `request_unsupported`, `provider_not_configured`, `prompt_required`,
  `range_not_satisfiable` (416).
- `health.check()` resolves to the report a 503 carries, as to a 200's: the
  server answers 503 when a provider answered unhealthy (`status`
  `"degraded"`, with what each vendor answered), and the SDK used to throw
  it as a `LiteGenAPIError`. A 503 without that report (litegen's own error,
  or something in front of the gateway) still throws, as does any other
  error.
- `keys.patch` clears what it is sent as `null`: `token_quota`,
  `rpm_limit`, `webhook_url` and `expires_at`, where `null` used to change
  nothing. A field left out still stays as it is, and a `webhook_url` of
  `""` is stored as no webhook.
- Webhooks: every delivery request carries `X-Litegen-Delivery-Id`, `whd-`
  and a UUID, the same on every attempt of one delivery. Delivery is at
  least once, so a receiver drops a repeat by that id; the signature covers
  the body, not this header. A job's webhook is saved with its outcome, so a
  restart loses none; a failed attempt is retried for about a day (13
  attempts) unless the receiver answers `410 Gone`, and each attempt is
  built when it is sent, so its signed links are fresh.
- Reference roles a model takes apart (`RefRoleSpec.not_with`, on every
  model whose vendor takes one or the other, such as Vidu's first and last
  frames or reference images) are refused together with `ref_role_conflict`
  (a `LiteGenAPIError` with `param: "reference_images"`) before anything is
  quoted or reserved. Where such a model's last frame also needs a first
  frame, a last frame sent with reference images is now `ref_role_conflict`
  (it was `ref_role_required`). `needs_first_frame` now also marks a
  `reference` role: Runway's Kling O3 models take reference images only from
  a first frame, so references without a `first_frame` (or `init`) image are
  refused with `ref_role_required` before the quote.
- A generation that keeps nothing (`keepOutputs: false`) converts nothing,
  so it is promised only what its vendor job returns (`returned_together`,
  else the one container asked for): asking for more is a 400
  `format_needs_keeping`. Its 3D files are the vendor's own links, each
  checked with a one-byte ranged request rather than downloaded;
  `size_bytes` is then the size the vendor's host reported, and
  `generations.assets()` lists them with `kept: false` (it said `true`).
- `images.submit` on a model route: the `wait` it asks for (`Prefer`) is
  honoured only when every deployment's vendor runs jobs, and it then also
  ends a deployment's call early (at most the deployment's
  `timeout_seconds`), resolving to the generation to come rather than
  waiting out the deployment's whole timeout.
- `admin.providers` answers show an `api_base` with user info as
  `https://[redacted]@host/v1`, and the URLs in `options` likewise. Sent
  back unchanged to `configure` or `patch`, such a URL keeps the one in
  force; any other `[redacted]` user info is a 400
  `invalid_provider_config`. `extra_headers` values are never shown, only
  their names (`extra_header_names`).
- Signing up, inviting and accepting an invitation that makes an account
  check that the email is an address: a 400 `invalid_email` otherwise.
- `LiteGenAPIError.param` names the field at fault on a `malformed_request`
  (a body the server can't read, such as a negative `feather_px`), and the
  message starts with that path; `param` was absent.

### Deprecated

- The `_dev_token` that `LITEGEN__DEV__EXPOSE_INVITE_TOKENS` adds to an
  invitation's answer: read `token` (or `invite_url`), which every answer
  carries.

### Fixed

- A test webhook (`keys.testWebhook`) is signed like a job's webhook, so a
  receiver that checks signatures accepts it.
- In single-tenant mode the master key may list and invite users
  (`users.list`, `users.invite`), where it got a 403.
- A hosted `audit.list` with the master key answers the entries filed under
  no org, where it was a 403 `no active organization`.

## [0.1.0] - 2026-10-30

### Added

- `VERSION`, this package's version.
- Response headers: the `onResponse` client option sees every response,
  errors included; `readResponseMeta(headers)` reads litegen's headers (what
  lax mode dropped, the rate limit, an idempotent replay, `Retry-After`); and
  `LiteGenAPIError.headers` carries an error response's.
- `idempotencyKey` on every billed submit (`images.generate`,
  `videos.generate`, `models3d.generate`): the same key with the same request
  replays the first answer, billed once.
- `generations.assets(id)` and `generations.downloadAsset(id, index)`: every
  kept result, read with a litegen key.
- `generations.cancel(id)` resolves to a `CancelledGeneration`: the row as
  settled (refunded, so `cost_usd` 0) and `vendor_cancelled`.
- `metadata` on every generation and segmentation request, echoed on every
  answer; `logs.list({ metadata })` and `stats.get({ metadata })` filter by it.
- `images.segment`, background removal, partial edits (`region`, masks,
  `mask_prompt`, `layout`, `expand`, `preserve_unmasked`), native pixel art
  (`palette`, `directions`), output format and transparency.
- `models3d`: rigging, animation, texture, remesh, segment, placement,
  multiview-to-3D, `source_model`, and the measured `scene` manifest.
- Video: timed keyframes, sprite controls (`camera`, `audio`, `loop`),
  `extend`, `retake` and `video_edit` on a `source_video`, with `segments`.
- Vendor errors carry the vendor's own answer (`providerError`) and whether to
  retry (`retryable`); every error `code` is a string.

### Changed

- A request now waits `DEFAULT_TIMEOUT_MS` (6.5 minutes) by default, not
  60 s: a synchronous call can take three vendor attempts of up to 120 s.
- Polling now waits `JOB_DEFAULT_TIMEOUT_MS` (2 hours 5 minutes) for every
  job, not 5 minutes (30 for 3D): the server fails a job still running after
  2 hours itself. `MODEL3D_DEFAULT_TIMEOUT_MS` equals it.
- `stats.get` takes `(opts?, signal?)`; `stats.get(signal)` still works.

### Fixed

- An already-aborted signal no longer sends the request.
- A poll rides out transient failures (a 429, a 5xx, a dropped connection)
  instead of throwing on the first.

## [0.0.1]

Unpublished first cut, copied from source.
