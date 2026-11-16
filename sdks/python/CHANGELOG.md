# Changelog

Every release of `litegen-sdk`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor release may change behaviour, and says how here.

## [Unreleased]

### Added

- `health.ready()` on the sync and async clients: the readiness probe
  (`GET /health/ready`, no credential needed), `{"status", "checks": {"db",
  "providers"}}`, as the TypeScript SDK's `health.ready()`. While the server
  is not ready, the 503 raises `LiteGenAPIError` (`status` 503) like any
  error.
- In the generated client: `MeResponse`, `MeOrg` and `MeUser`, so
  `api.auth.me` parses the signed-in person (each organization with the
  `role` and the `permissions` held there, as the server checks them;
  `active_org` and `active_app`) instead of returning nothing; and
  `RefRoleSpec.needs_first_frame`: on a video model's `last_frame` role, a
  last frame sent without a `first_frame` (or `init`) image is refused with
  `ref_role_required` (`param` `reference_images`) before anything is quoted.
- `package="zip"` on `models3d.convert`: the answer is a zip
  (`content_type` `application/zip`, `filename` `model.<to>.zip`) holding the
  converted file first and whatever goes beside it, such as an OBJ's `.mtl`
  and textures. The SDK returns the zip unopened.
- `ktx2_textures` on `models3d.convert`: `"png"` (the default) re-encodes every
  KTX2 (Basis Universal) texture as PNG, and `"keep"` leaves it as it is when
  `to_format` is `"glb"` or `"gltf"`, which then require `KHR_texture_basisu`.
- `ktx2_textures` on 3D generation requests, and its type
  `Model3dKtx2Textures`: the same choice for delivery. `"keep"` on a request
  that keeps nothing is refused (`option_needs_keeping`).
- `fbx_light_units` and `usd_light_units` on `models3d.convert`: the units FBX
  and USD light strengths are stored in at that end, `"photometric"` (the
  default; glTF's candela and lux) or `"blender"` (so the lights open in
  Blender as bright as the glTF does).
- `fbx_light_units` and `usd_light_units` on 3D generation requests, and
  their type `Model3dLightUnits`: the same choice, for when litegen reads the
  vendor's FBX or USD or writes one for the request.
- `dae_morphs` on `models3d.convert`, when `to_format` is `"dae"`: `"bake"`
  (the default) writes each mesh at its weights and drops the morph targets'
  animation, which every Collada reader opens; `"keep"` writes the targets as
  `<morph>` controllers with their weight animation, which SceneKit (Xcode)
  and Godot read and three.js and assimp refuse.
- `dae_morphs` on 3D generation requests, and its type `Model3dDaeMorphs`:
  the same choice for a Collada litegen writes for the request. `"keep"` on a
  request that keeps nothing is refused (`option_needs_keeping`).
- `"usdc"` in `Model3dFormat`: a USD layer written as a binary crate, for
  `models3d.convert`'s `from_format` and `to_format` and for
  `output_formats`. `"usda"` and `"usdc"` each read either encoding.
- `usdz_root` and `usd_clips` on `models3d.convert`. `usdz_root` (when
  `to_format` is `"usdz"`) writes the package's root layer as `"usda"` text
  (the default) or a `"usdc"` crate, as Apple's own packagers do. `usd_clips`
  (when `to_format` is `"usda"`, `"usdc"` or `"usdz"`) keeps `"all"` clips as
  the variant set `Animation` or the `"first"` alone; by default all for usda
  and usdc, the first for usdz.
- `usdz_root` and `usd_clips` on 3D generation requests, and their types
  `Model3dUsdzRoot` and `Model3dUsdClips`: the same choices for a USD litegen
  writes for the request. `"usdc"` and `"all"` on a request that keeps
  nothing are refused (`option_needs_keeping`).
- `output_packages` on 3D generation requests, and the `"package"` asset kind
  it delivers: one zip per requested container that has files to go beside
  its mesh.
- `FramesSetShape` in `litegen.types`, the values of `frames_set_shape` on a
  video model's `params.aspect_ratio`: whether a `first_frame`, `last_frame`
  or `init` image sets the clip's shape, so `aspect_ratio` is refused (or
  dropped in lax mode) beside one.
- In the generated models: `ModelSchema.litegen_params` (read-only on the
  server: the fields litegen applies to the model itself, in the `ParamSpec`
  shape), `extra_params` (the types of its `extra_allowlist` keys) and
  `status`; `ModelCapabilities.max_request_bytes` (required, as it is on
  every listed model, so the generated class takes it and its `from_dict`
  needs a server that sends it);
  `PromptSpec.required_for_text_to_video`;
  `ParamSpecAspectRatio.frames_set_shape`; `ProviderHealth.modality`
  (required, so the generated `from_dict` needs a server that sends it);
  `GatewaySettings.cache_max_bytes` and `EffectiveCache.max_bytes`;
  `ImageSegmentationRequest.strict`. A schema `models.get` served may be posted
  back to `admin.models.create` or `replace` as it came.
- `keys.patch(key_id, **fields)` on the sync and async clients
  (`PATCH /v1/keys/{id}`), typed by `UpdateApiKeyRequest`, returning the key
  as an `ApiKeyDetail`. It is a JSON merge patch: a field given is set, a
  field left out stays as it is, and `None` is sent as `null`, which clears
  `token_quota`, `rpm_limit`, `webhook_url` (`""` clears it too) and
  `expires_at`. `name`, `scopes` and `is_active` can't be cleared.
- `keys.list_webhook_deliveries(key_id, page=1, per_page=50)`
  (`GET /v1/keys/{id}/webhook-deliveries`): the key's delivery attempts,
  newest first, as a `WebhookDeliveryPage`. Each `WebhookDelivery` carries
  its `delivery_id`, `whd-` and a UUID: the `X-Litegen-Delivery-Id` its
  receiver got, the same on every attempt of one delivery (`None` on
  attempts recorded before migration 029). Every attempt of one delivery
  shows the payload of its latest attempt.
- `keys.redeliver_webhook_delivery(key_id, delivery_id)`
  (`POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver`): sends
  a delivery that ended (`delivered` or `failed`) again, as a fresh series
  of attempts under the same `delivery_id`, to the key's `webhook_url` of
  that moment, and returns its `WebhookDeliveryState` (`status` a
  `WebhookDeliveryStatus`: `pending`, `delivered` or `failed`). It raises a
  409 `delivery_pending` while the delivery is still being sent, a 429
  `redeliver_too_soon` within a minute of its last redeliver (`retry_after`
  says when to ask again), a 400 `no_webhook_configured` or
  `invalid_webhook_url`, and a 404 for an unknown key or delivery.
- `audit.list(...)` (`GET /v1/audit`, `audit:read`), an `AuditLogPage`
  filtered by `actor_key_id`, `action`, `from_` and `to`. The master key
  reads another org's entries with `org_id`; anyone else may name only
  their own (else a 403). `AuditLogEntry.org_id` is absent from an entry
  filed under no org (in hosted mode, the master key's changes to the
  gateway and to accounts, and an account change by a person in no org);
  only the master key reads those, by naming no `org_id`.
- `admin.providers.patch(name, **fields)`
  (`PATCH /v1/admin/providers/{name}`), typed by
  `PatchProviderConfigRequest`, returning a `ProviderConfigView`: it changes
  some of a provider's runtime config and keeps the rest. A field given is
  set, `None` clears it (it falls back to env/YAML) and a field left out
  keeps its stored value. A secret (`api_key`, `api_keys`, `key_id`,
  `key_secret`) left out or `None` keeps its stored value, and one sent as
  `""` removes it, so the env/YAML one applies again; a `model_mapping`,
  `extra_headers` or `options` given replaces the stored one whole.
  `configure` (`PUT`) still replaces the whole config.
- `orgs.members.invite(org_id, email=..., role=...)`
  (`POST /v1/orgs/{id}/members`), returning the new invitation as an
  `InviteResponse`. Its `token` and `invite_url` are shown only in this
  answer, and litegen sends no email, so hand one of them to the invitee;
  `invite_url` is `None` when the server has no `server.public_base_url`
  (build `/invite/{token}` on the dashboard's origin). A 400 is
  `cannot_invite_owner` or `invalid_email`.
- `orgs.invitations.list(org_id)` (`GET /v1/orgs/{id}/invitations`): the
  org's invitations not yet accepted or expired, newest first, as
  `PendingInvitation`s without their tokens; and
  `orgs.invitations.revoke(org_id, invitation_id)`, which returns `None`
  (404 `invitation_not_found` when no invitation with that id waits in the
  org). To send a lost link again, revoke the invitation and invite anew.
  The `orgs` methods take the master key or a signed-in person's session;
  an API key gets a 403 `forbidden`.
- `users.password_reset_link(user_id)`
  (`POST /v1/users/{id}/password-reset`), returning a `PasswordResetLink`.
  Its `token` and `reset_url` (`None` when the server has no
  `server.public_base_url`) are shown only in this answer and set the
  account's password once, until `expires_at` (an hour); litegen sends no
  email. The master key may, and so may a signed-in person who outranks the
  account in every org it belongs to, never for their own account; for a
  deactivated account only someone who may reactivate it, since confirming the
  link reactivates it. An API key gets a 403. A 403 is
  `forbidden_permission`, a 404 `user_not_found`.
- `LiteGenAPIError.body`: the error response's body parsed as JSON, when it
  was JSON (what the server answered with the status), else `None`.
- Typed answers: `logs.list` returns a `RequestLogPage`, `spend.logs` a
  `SpendLogPage`, and `admin.settings.get` and `update` a
  `GatewaySettingsView` (its `settings` a `GatewaySettings`, with
  `AccessSettings`, `EffectiveCache`, `EffectiveRetries`, `EffectiveBreaker`,
  `GatewaySettingLimits` and `SettingRange`, and the `AccessMode`,
  `OwnerScope` and `SignedLinks` literals); `admin.settings.update` takes
  `GatewaySettings`' keys. Request and spend log rows carry `cache_hit`
  (whether the cache answered; a hit has a row of its own, at what it was
  charged), and the settings `cache_charge_hits`, in force `charge_hits`:
  whether a hit is charged the cached answer's price (by default it is;
  `False` makes hits free).
- In `litegen.types`, held to `sdks/openapi.json` like the 3D types:
  `UpdateApiKeyRequest`, `ApiKeyDetail`, `WebhookDelivery`,
  `WebhookDeliveryPage`, `WebhookDeliveryState`, `WebhookDeliveryStatus`,
  `AuditLogEntry`, `AuditLogPage`, `PatchProviderConfigRequest`,
  `ProviderConfigView`, `InviteResponse`, `PendingInvitation`, `Role`,
  `PasswordResetLink` and the log and settings types above.
- In the generated client: `admin_patch_provider`, `list_invitations`,
  `revoke_invitation`, `redeliver_webhook_delivery` and
  `issue_password_reset`, with the models `PatchProviderConfigRequest`,
  `PendingInvitation`, `WebhookDeliveryState`, `WebhookDeliveryStatus` and
  `PasswordResetLink`; `RequestLog.cache_hit`, `SpendLogEntry.cache_hit`,
  `GatewaySettings.cache_charge_hits` and `EffectiveCache.charge_hits`;
  `AuthConfigResponse.signup_open` (whether self-service signup is open:
  always in hosted mode, in single-tenant mode while no account exists) and
  `AuthConfigResponse.password_min_length` (the shortest password signup,
  invitations, password changes and resets accept), both required, so the
  generated `from_dict` needs a server that sends them; `InviteResponse.org_id`, `token` and `invite_url`
  (required, so the generated `from_dict` needs a server that sends them;
  `invite_url` may be `None`), so `invite_member` parses its 200 (it
  returned nothing) and `invite_user`'s answer carries the token;
  `list_audit`'s `org_id`; `WebhookDelivery.delivery_id`;
  `RefRoleSpec.not_with` (see Changed);
  `ParamSpecStringArray.returned_together`, on `output_formats`: the
  containers one vendor job returns together (Meshy's rig returns a glb and
  an fbx at once); `GatewaySettings.encrypt_provider_credentials`, which
  also encrypts each `extra_headers` value and the user info of `api_base`
  and of the URLs in `options` at rest (off by default; turning it on needs
  `LITEGEN__SECRETS_KEY`, else a 400 `secrets_key_missing`; unlike the other
  settings, a PUT that leaves it out keeps its stored value);
  `GatewaySettingsView.limits` (`GatewaySettingLimits`: a `SettingRange`,
  `min` and `max`, both allowed, for each numeric setting, as
  `PUT /v1/admin/settings` checks them); `health_check` parses the 503
  (`status` `"degraded"`: a provider answered unhealthy) as a
  `HealthResponse`; and the file routes document their 410
  (`files_deleted`, `vendor_file_gone`), 500, 502
  (`vendor_file_unreachable`) and 503 (`hook_failed`) answers, the signed
  route its 307.

### Changed

- `AnimationRequest.name` is `NotRequired[str | None]`: litegen names a clip
  that leaves it out (`walk` after its preset, else `clip-<n>`).
- What `X-Litegen-Dropped-Params` covers: fields the request has no such name
  for too, one inside an object by its path, on the quotes, `images.segment`
  and the image 202 as well. It lists at most 32 names and ends with a
  `+N more` count past that; `dropped_params` and `response_meta` keep that
  entry.
- `LiteGenAPIError.retryable` comes with every router error, such as a 503
  `circuit_open` (every deployment of a model route cooling down, with
  `retry_after`), not only a vendor's. New codes callers may see:
  `circuit_open`, `cache_clear_incomplete` (`cache.clear`, 503),
  `request_unsupported` and `prompt_required` (both `LiteGenValidationError`),
  `provider_not_configured`, `range_not_satisfiable` (416).
- `health.check()` returns the report a 503 carries, as it returns a 200's:
  the server answers 503 when a provider answered unhealthy (`status`
  `"degraded"`, with what each vendor answered), and the SDK used to raise
  it as a `LiteGenAPIError` whose message held the report's JSON. Any other
  error still raises, a 503 that is litegen's `{"error": …}` included.
- The key PATCH clears what it is sent as `null`: `token_quota`,
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
  (`param` `reference_images`) before anything is quoted or reserved. Where
  such a model's last frame also needs a first frame, a last frame sent with
  reference images is now `ref_role_conflict` (it was `ref_role_required`).
  `needs_first_frame`
  now also marks a `reference` role: Runway's Kling O3 models take reference
  images only from a first frame, so references without a `first_frame` (or
  `init`) image are refused with `ref_role_required` before the quote.
- A generation that keeps nothing (`keep_outputs=False`) converts nothing,
  so it is promised only what its vendor job returns (`returned_together`,
  else the one container asked for): asking for more is a 400
  `format_needs_keeping`. Its 3D files are the vendor's own links, each
  checked with a one-byte ranged request rather than downloaded;
  `size_bytes` is then the size the vendor's host reported, and
  `generations.assets()` lists them with `kept: False` (it said `True`).
- `images.submit` on a model route: the wait it asks for (`Prefer`) is
  honoured only when every deployment's vendor runs jobs, and it then also
  ends a deployment's call early (at most the deployment's
  `timeout_seconds`), answering with the generation to come rather than
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
  message starts with that path; `param` was `None`.

### Deprecated

- The `_dev_token` that `LITEGEN__DEV__EXPOSE_INVITE_TOKENS` adds to an
  invitation's answer: read `token` (or `invite_url`), which every answer
  carries.

### Removed

- The `additional_properties` bag of nine generated models, whose schemas
  now refuse unknown keys (`additionalProperties: false`): `ModelSchema`,
  `CapabilityModelPricing`, `ModelCapabilityFlags`, `PromptSpec`,
  `RefInputSpec` and `RefRoleSpec` (also re-exported at `litegen.*`), and
  `NativeOrientation`, `AnimationPresetSpec` and `TextToMotionSpec` in
  `litegen._generated.models`. They lose `additional_properties`,
  `additional_keys`, `obj[key]`, `obj[key] = value`, `del obj[key]` and
  `key in obj`; `from_dict` drops a key the schema does not name instead of
  keeping it, and `to_dict` sends declared fields only.

### Fixed

- A test webhook (`POST /v1/keys/{id}/test-webhook`) is signed like a job's
  webhook, so a receiver that checks signatures accepts it.
- In single-tenant mode the master key may list and invite users (`GET` and
  `POST /v1/users`), where it got a 403.
- A hosted `GET /v1/audit` with the master key answers the entries filed
  under no org, where it was a 403 `no active organization`.

## [0.1.0] - 2026-10-30

### Added

- Response headers: the `on_response` client option is called with every
  `httpx.Response`, errors included; `response_meta(headers)` reads litegen's
  headers (what lax mode dropped, the rate limit, an idempotent replay,
  `Retry-After`); and `LiteGenAPIError.headers` carries an error response's.
- `idempotency_key` on every billed submit: the same key with the same
  request replays the first answer, billed once.
- `generations.assets(id)` and `generations.download_asset(id, index)`.
- `generations.cancel(id)` returns a `CancelledGeneration`: the row as
  settled (refunded, so `cost_usd` 0) and `vendor_cancelled`.
- `metadata` on every generation and segmentation request, echoed on every
  answer; `logs.list(metadata=...)` and `stats.get(metadata=...)` filter by it.
- `images.segment`, background removal, partial edits, native pixel art,
  output format and transparency; `models3d` rigging, animation, operations,
  placement, multiview and `source_model`; video keyframes, sprite controls
  and source-video operations.
- Vendor errors carry the vendor's own answer (`provider_error`) and whether
  to retry (`retryable`).

### Changed

- A request now waits `DEFAULT_TIMEOUT` (390 s) by default, not 60 s: a
  synchronous call can take three vendor attempts of up to 120 s.
- Polling now waits `JOB_DEFAULT_TIMEOUT` (2 hours 5 minutes) for every job,
  not 300 s (1800 for 3D): the server fails a job still running after 2 hours
  itself. `MODEL3D_DEFAULT_TIMEOUT` equals it.
- The package version is 0.1.0 in `pyproject.toml` as in `__version__`.

### Fixed

- A failed submit is never sent again.
- A poll rides out transient failures instead of raising on the first.

## [0.0.1]

Unpublished first cut, copied from source.
