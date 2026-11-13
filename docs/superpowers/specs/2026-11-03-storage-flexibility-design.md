# Storage flexibility — design (known gaps, round 2)

Status: on 2026-11-03 the operator asked to "fix the gaps to ensure our app has
full flexibility so users can use it however they need to like no storage
option should definitely be an option". The design calls are mine: on
2026-11-02 the operator handed them over ("research best practices and most
needed use cases … build it in the best way u can with flexibility for
customiznig it per use case").

The gaps are fixed in four rounds, in order of how much they change what users
can do:

1. **Storage** (this document): keep nothing, retention, deletion, revoking
   links, app buckets through an IAM role, where files go when an app's bucket
   is down, and the storage bugs left from 2026-11-02.
2. **Provider config:** `extra_headers` and `model_mapping` in every adapter,
   the legacy auth headers, numeric value sets, unknown vendor states, the
   remaining 429s, Luma image's poll, `extra` merges, `init` start-frame rows.
3. **Delivery and billing:** images that finish after the timeout, and pricing
   by variant.
4. **Surfaces:** README, landing data, price API, clippy, providers with no
   models, and the remaining minor bugs.

## Goals, non-goals, anti-goals

- **Goals:**
  - A deployment, an app or one request can keep nothing.
  - Kept files can be deleted: after a set time per app, one generation on
    request, and an app's or org's files when it is deleted.
  - Links can be revoked.
  - An app's bucket can be reached through a cross-account IAM role.
  - An app can refuse to have its files land in litegen's own store.
- **Non-goals:** retention of generation rows (billing records stay), a
  per-end-user retention, and object-lock or legal-hold features.
- **Anti-goals:**
  - A setting that silently weakens another. For example, a private app whose
    files become public because it also keeps nothing.
  - A billed result lost to a keep setting.
  - Any change for a deployment that sets nothing: every default is today's
    behaviour.

## The settings

They join `storage_access` and are layered like its other fields, each one on
its own. The app's value comes first (`/v1/apps/{id}/access`), then the
gateway's (`/v1/admin/settings`), then `litegen.yaml`'s.

| setting | yaml (`storage_access.`) | gateway field | values | default |
|---|---|---|---|---|
| keep outputs | `keep_outputs` | `storage_access_keep_outputs` | `true`, `false` | `true` |
| delete kept files after | `retention_days` | `storage_access_retention_days` | 0 to 36500; 0 keeps them until deleted | 0 |
| who signs a signed link | `signed_links` | `storage_access_signed_links` | `store`, `litegen` | `store` |

- **A bad value refuses to start litegen** (yaml) or answers 400 (API), as the
  existing fields do: `invalid_keep_outputs`, `invalid_retention`,
  `invalid_signed_links`.
- **A request can keep less than its app, never more.** The header
  `X-Litegen-Keep-Outputs: false`, on any generation request, keeps nothing
  for that call. Any other value answers 400 `invalid_keep_outputs_header`.

## 1. Keep nothing

`keep_outputs: false` means litegen writes no output file anywhere: not to an
app's bucket, not to litegen's store, not to the image cache. The decision is
taken when the request arrives, from the policy and the header. For video and
3D it is saved on the row (`metadata.keep_outputs: false`), so the observer
that sees the job finish honours it, even after a restart. The row itself is
still written: model, cost, status, request.

- **Images** come back inline, as `b64_json`, whatever `response_format`
  says, with `url: null`. Adapters already return the bytes. Nothing is
  uploaded to an S3 image store, no history copy is written, and the answer is
  neither looked up in the cache nor stored there (`X-Litegen-Cache: bypass`).
  The row records no assets.
- **Video:** the vendor's link becomes the asset, unchanged, with
  `kept: false`. Nothing is downloaded or stored, and `media` stays empty.
  - All real video adapters return a link. A vendor that returns only bytes
    (today only the mock's GIF video) has nothing to hand out, so that
    generation fails at once: refunded, webhooked, with a message that names
    the setting.
- **3D:** the vendor's own files are handed out as `kept: false` assets, with
  the URLs the adapter downloaded them from (`Model3dFile.source_url`, which
  fal sets). There is no conversion, no normalisation and no scene manifest.
  - A request asking for a format the vendor doesn't make itself answers 400
    `format_needs_keeping`, before any vendor call, and lists the formats the
    endpoint makes. Conversion writes a file, and keeping nothing rules that
    out.
  - Files without a source URL fail the generation, as video bytes do.
- **How links are handed out** follows the access mode, so keeping nothing
  never opens anything:
  - `public`: the vendor's link;
  - `signed`: a litegen link (section 2), and litegen streams the vendor's
    file while the vendor still has it;
  - `private`: the file route, which streams the vendor's file after the
    owner check and the `file_access` hook.

  The vendor's link never leaves litegen in `signed` or `private` mode. A
  vendor that has dropped the file answers 410 `vendor_file_gone`.
- **Inputs are not outputs.** An uploaded reference image is still published
  for the vendor to fetch, and deleted within the hour (`ref_publish`). The
  docs say so.

## 2. Signed links through litegen

A signed link today is minted by the store that holds the file: an S3
presigned GET, or the local store's HMAC link. Neither works for a vendor's
file, and an S3 link can't be revoked. So litegen gets its own signed link,
to the file route:

`GET /v1/generations/{id}/assets/{index}?expires=…&issued=…&signature=…`

- **The signature** is the `LinkSigner` HMAC over
  `generation-file:{id}/{index}@{issued}` and `expires`. The prefix keeps it
  apart from local-store keys.
- **A valid, unexpired, unrevoked link opens the file without credentials,**
  skipping the owner check and the hook, as every signed link does today.
  Otherwise the route answers 403 with `invalid_link`, `link_expired` or
  `link_revoked`. Without a signature the route works as today.
- **When the presenter mints one,** in `signed` mode for a caller in the owner
  scope:
  - always, with `signed_links: litegen`;
  - with `signed_links: store`, whenever no store can sign the file: vendor
    links, an S3 image store's URLs, or a bucket reached through a role whose
    session ends before the link would (section 6). Today such a file falls
    back to the file route, which needs credentials, so this case improves.
- **The file route then serves the file** from whichever store holds it, or
  streams the vendor's file.

## 3. Revoking links

- **A `link_revocations` table** holds rows of `(scope, scope_id, revoked_at)`,
  where scope is `gateway`, `app` or `generation`. A litegen link (section 2)
  or a local-store link issued before the latest `revoked_at` among its
  gateway, its app and its generation is refused, at once, on every replica.
- **Every local-store link now carries `issued`.** A link minted before this
  change has none and counts as issued at 0, so any revocation ends it.
- **The routes:**
  - `POST /v1/apps/{app_id}/access/revoke-links`, needing the storage
    permission;
  - `POST /v1/generations/{id}/revoke-links`, needing
    `generation:delete:own|any`;
  - `POST /v1/admin/revoke-links`, for the gateway admin.

  Each answers `{ "revoked_at": … }` and is audited. `GET /v1/apps/{id}/access`
  shows the app's `links_revoked_at`.
- **What can't be revoked:** an S3 presigned link (`signed_links: store` on a
  bucket) works until it expires. The docs, the API description and the
  dashboard say so, and point to `signed_links: litegen`.

## 4. Retention

- **`generations.files_deleted_at`** is a new column, and it shows on every
  generation answer.
- **The sweep:**
  - Every replica sweeps hourly. Finished generations whose app's effective
    `retention_days` is above 0, older than that, and with files not yet
    deleted, are swept 500 at a time.
  - Each stored asset is deleted from the store that holds it (the app's
    bucket or litegen's own); then the row is marked.
  - Deleting first and marking second means a failed delete is retried on the
    next sweep. Two replicas deleting the same file is harmless.
  - `kept: false` assets have nothing to delete.
- **Afterwards** the file route answers 410 `files_deleted` with the reason
  (retention or a request), and the row's other fields stay.
- **litegen's local store keeps its own cap,** the file-age sweep of
  `image_storage.retention_days` (at least 7 days), whatever an app sets. The
  docs say so, and the dashboard warns when an app asks for longer.

## 5. Deleting

- **`DELETE /v1/generations/{id}`** needs a new `generation:delete:own|any`,
  granted to the roles that hold `generation:cancel:own|any`. It answers 409 for
  a job still running ("cancel it first").
  - It deletes the stored files, sets `files_deleted_at` and removes
    `metadata.request` (the prompt and params).
  - It answers with the generation as it now stands. The billing record stays.
- **Deleting an app or an org** deletes its files in litegen's own store, in
  the background, after collecting their keys and before the rows go. Files in
  the app's own bucket are the customer's, so they stay, and the docs say so.

## 6. App buckets through an IAM role

AWS's pattern for a third party reaching a customer's account: the customer
makes a role that trusts the third party's account, on condition of an
external ID that the third party assigns. The customer never picks the
external ID, which prevents the confused-deputy problem.

- **`PUT /v1/apps/{id}/storage`** takes `credential_mode`:
  - `access_key`: today's mode, and the default;
  - `assume_role`: with `role_arn` (checked against
    `arn:aws[a-z-]*:iam::\d{12}:role/…`).

  For `assume_role`, the PUT assumes the role once and answers 400
  `assume_role_failed` with STS's message if that fails.
- **`GET /v1/apps/{id}/storage`** always shows the app's `external_id`, so
  the trust policy can be written before the role is saved:
  - The external ID is `litegen-` followed by 32 hex characters of
    HMAC(secrets key, `aws-external-id:{app_id}`). It is stable and needs no
    storage.
  - The GET also shows `litegen_principal_arn`, from STS
    `GetCallerIdentity`, cached; `null` when litegen has no AWS identity.
- **litegen's own AWS identity** comes from a new `aws` config section:
  `access_key_id`, `secret_access_key`, `session_token`, `region` (default
  us-east-1) and `sts_endpoint` (default `https://sts.{region}.amazonaws.com`).
  When it is unset, the standard chain applies (env, profile, instance or
  container metadata, web identity), through `aws-creds` on a blocking thread.
- **The STS call** is a SigV4-signed `AssumeRole` (session
  `litegen-{app_id}`, at most 64 chars; 3600 s). It is written in-crate with
  the existing hmac/sha2 and `quick_xml`, and checked against AWS's published
  SigV4 example.
  - Credentials are cached process-wide per role and external ID until 5
    minutes before they expire.
  - They are passed to rust-s3 with their session token.
- **A presigned link can't outlive the session that signed it.**
  `S3Storage::signed_url` returns `None` when the credentials expire before
  the link would, and the presenter then mints a litegen link (section 2).
- **The columns** `credential_mode` and `role_arn` are added to the app
  storage table. The secret blob is unused in role mode.

## 7. When an app's bucket can't be reached

Today a broken app bucket config (decrypt, build or role failure) quietly
falls back to litegen's own store, so a tenant's files can land outside its
bucket. A new app storage field, `fallback_to_litegen_store` (default `true`,
today's behaviour), makes this a choice. With `false`:

- an image's history copy is skipped, with a warning, and the caller still
  has the image;
- a video or 3D keep step fails as transient, so it is retried and then
  reaped (refund, webhook);
- nothing reaches litegen's store.

## 8. Storage bugs from 2026-11-02

- **A cached image answer in a private app with an S3 image store hands out
  the bucket URL.** On a cache hit, the presenter maps the answer through the
  original generation's row, which exists, instead of a failed insert.
- **List endpoints resolve each row's policy and bucket separately.** They now
  resolve them once per app per call.
- **The dashboard's access panel shows the previous app's settings** after the
  app switcher clears the active app.
- **A GIF video previews as `<video>` in private mode,** because the
  extension test misses file-route URLs. The asset's `format` is used instead.

## 9. Dashboard, SDKs, docs

- **Dashboard:**
  - Organization → stored files gets keep outputs, retention, signed links,
    and "Revoke links" with the last time.
  - The storage form gets the credential mode, the role ARN, the external ID
    and litegen's principal, plus the fallback.
  - Gateway → Settings → Stored files gets the three gateway fields and
    "Revoke all litegen links".
  - A generation's detail view gets "Delete files" where one exists.
- **SDKs (TS and Python):** methods for the new routes and types for the new
  fields, following the contract test. No allowlist growth.
- **OpenAPI:** ping the peer session before landing it, as before.
- **Docs:**
  - `docs/storage-access.md` becomes the guide to keeping, retention,
    deleting, revoking and links;
  - the README's BYO storage section, `docs/gateway-settings.md`, the skills
    and `known-gaps.md` are updated to match.

## 10. Proof against a real S3 server

MinIO can't be obtained on this machine (Docker Hub refused, `dl.min.io` 410).
`s3s-fs` (the `s3s` project's S3 server over a directory, which verifies
SigV4, presigned links included) is tried through `cargo install` into the
scratchpad. If it runs, `tests/output_access_s3.rs` and the new retention and
deletion cases run against it. If it doesn't, the reason is recorded and the
gap stays listed.

## Left to the builder

- Function and module names, and the exact log and error wording.
- Streaming the vendor's file versus buffering it, under the existing size
  cap.
- The sweep's batch query shape, as long as it uses the indexes.
- Whether the dashboard's warnings are inline text or a callout.
