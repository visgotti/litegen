# Stored outputs: keeping, links and access

By default litegen keeps every result it produces: images, videos, 3D meshes
and the formats it converts them to. They live in your app's own bucket (see
[Your app's own bucket](#your-apps-own-bucket)) or in litegen's store. This
page covers:

- keeping nothing;
- how long kept files stay, and deleting them;
- how the URLs litegen hands out work, and revoking them;
- who can open the files.

The short version:

- **keep** outputs or not, and for how long (**retention**);
- a **mode** for how links work, and who signs them;
- an **owner scope** for which keys may open a file through litegen.

Set gateway-wide defaults in `litegen.yaml` or on the dashboard, and override
them per app. One request can keep nothing with `X-Litegen-Keep-Outputs: false`.

## Keeping nothing

`keep_outputs: false` means litegen writes no output file anywhere: not to an
app's bucket, not to litegen's store, not to the image cache, not to the
request log. You need no bucket and no disk for outputs.

- **Images** come back inline, as `b64_json`, whatever `response_format` says,
  with `url: null`. Nothing is uploaded, and the cache is bypassed
  (`X-Litegen-Cache: bypass`).
- **Video** answers carry the vendor's own link. Nothing is downloaded, and
  `media` stays empty. A model whose vendor returns the video's bytes, or a
  link only it can read (Google Veo, Bedrock's `s3://` output), has nothing
  to hand out: a request that keeps nothing is refused, 400
  `keep_outputs_unsupported`, before the vendor is called.
- **3D** answers carry the vendor's own files, unconverted and
  unnormalised, as assets with `kept: false`. litegen makes other formats by
  converting a file it keeps, so a request for a format the vendor doesn't
  make itself answers 400 `format_needs_keeping`, before any vendor call, and
  names the formats the vendor makes.
- **The access mode still decides how a vendor's file is handed out,** so
  keeping nothing never opens anything a mode closes:
  - `public`: the vendor's link itself;
  - `signed`: a link litegen signs, which streams the vendor's file;
  - `private`: litegen's file route, which streams it after the owner check
    and your `file_access` hook.

  In `signed` and `private` the vendor's link never leaves litegen. Once the
  vendor deletes its file (anything from an hour to weeks), litegen answers 410
  `vendor_file_gone`.
- **The generation is still recorded,** with its model, cost, status and
  request, and `metadata.keep_outputs: false`. To remove its prompt too,
  delete it (see [Retention and deleting](#retention-and-deleting)).
- **One request can keep nothing** in an app that keeps outputs: send
  `X-Litegen-Keep-Outputs: false` (SDKs: `keepOutputs: false`,
  `keep_outputs=False`). A request can keep less than its app does, never
  more: any other value is a 400 `invalid_keep_outputs_header`.
- **The decision is taken when the request arrives** and saved on the row, so
  a change of setting while a video or 3D job runs doesn't change what that
  job does.
- **Not covered by keeping nothing:**
  - An uploaded reference image is still published for the vendor to fetch,
    and deleted within the hour.
  - An `Idempotency-Key` request stores its answer for the 24-hour replay,
    inline images included.
- **A client that hangs up** doesn't stop its request: it runs to its end
  and is charged if it succeeds. A synchronous image made while keeping
  nothing is then lost, since only the answer carried it.

## The three modes

| mode | what an answer carries | who can open the file | when to use it |
|---|---|---|---|
| `public` (default) | the URL the file was stored at | anyone holding the URL | public galleries, embeds, a CDN |
| `signed` | a link that works without credentials until it expires (1 hour by default) | anyone holding the link, until then | web and mobile apps that show results in `<img>`/`<video>` tags; handing a file to a third party |
| `private` | litegen's file route, `GET /v1/generations/{id}/assets/{index}` | only callers that pass the owner check, and your `file_access` hook if you have one | backends that fetch results with their API key; per-customer rules |

- **`public` is the default,** so a deployment that sets nothing answers
  exactly as before. For a new deployment we recommend `signed`.
- **Who signs a signed link** is `signed_links`:
  - `store` (default): the store that holds the file. A bucket signs its own
    (an S3 presigned GET, at most a week); litegen's local store signs with an
    HMAC over the file, the issue time and the expiry.
  - `litegen`: litegen, for every file. The link goes to
    `GET /v1/generations/{id}/assets/{index}/signed?expires=…&issued=…&signature=…`,
    which needs no credentials and serves the file from wherever it is.
  - Only litegen's own links can be revoked (see [Revoking links](#revoking-links)).
  - A file no store can sign (a vendor's file, or a bucket reached through a
    role whose session ends before the link would) always gets litegen's link.
- **The file route** needs a litegen key or a dashboard session. litegen
  streams the file from wherever it is stored, so the bucket can stay fully
  private.
- **Streaming and ranges.** A file litegen keeps (the file route, its signed
  link, and the asset route `/v1/models3d/assets/...`, whether the deployment
  stores on local disk or in its S3 bucket) is passed on as the store reads
  it, never read whole into memory, so a download costs the server a small
  buffer however large the file. These answers carry `Accept-Ranges: bytes`
  and honour one `Range: bytes=a-b`, `bytes=a-` or `bytes=-n`: 206 with
  `Content-Range`, or 416 (`range_not_satisfiable`) when the range starts
  past the end, with `Content-Range: bytes */<length>` when the length is
  known. Several ranges, or an `If-Range`, get the whole file (200). A file
  litegen didn't keep (`keep_outputs: false`) streams from the vendor whole.
  From the operator's own bucket, litegen reads with a presigned GET through
  its own HTTP client (no redirects, the environment's proxy, a stalled read
  cut off after 60 s), and refuses an object over 1 GiB, the most it keeps.
- **Whatever litegen serves from its own origin** (the file route, and the
  local store's asset route) carries `X-Content-Type-Options: nosniff` and
  `Content-Security-Policy: sandbox`. A browser that opens a file, an SVG
  for instance, can't run script from it as litegen.

## Who may open a file: the owner scope

Reading a generation (`GET /v1/generations/{id}`) is org-wide, as it always
was. **Opening its files** through litegen follows the owner scope:

| owner scope | API keys that may open a file |
|---|---|
| `key` (default) | only the key that made it |
| `app` | any key of the same app |
| `org` | any key of the same org |

- **Always allowed:** the master key, and dashboard sessions with their
  usual rights. `generation:read:any` opens any file; `generation:read:own`
  opens the files of your own keys.
- **In `signed` mode** a caller outside the scope still reads the generation,
  but its URLs are the file route, which refuses it. It never holds a working
  link to a file the scope keeps from it.
- **The scope covers the file route in every mode.** In `public` mode the
  stored URL itself is open to anyone, so the scope only guards what litegen
  serves.
- **No per-end-user rule is built in.** The key is the boundary. Per-customer
  rules belong in a `file_access` hook (below).

## Setting it

Each field is decided on its own: the app's setting, else the gateway's, else
`litegen.yaml`'s.

**In `litegen.yaml`** (or `LITEGEN__STORAGE_ACCESS__MODE` and friends):

```yaml
storage_access:
  mode: signed                  # public | signed | private
  owner_scope: key              # key | app | org
  signed_url_ttl_seconds: 3600  # 60 to 604800 (a week)
  keep_outputs: true            # false keeps nothing
  retention_days: 0             # 0 to 36500; 0 keeps files until they are deleted
  signed_links: store           # store | litegen
```

litegen refuses to start when `storage_access` is set but can't be read, for
example `mode: privat` or a TTL out of range. Falling back to its defaults
would hand out `public` links the operator meant to close.

| setting | `litegen.yaml` (`storage_access.`) | gateway field | app field | default |
|---|---|---|---|---|
| how links work | `mode` | `storage_access_mode` | `mode` | `public` |
| who may open a file | `owner_scope` | `storage_access_owner_scope` | `owner_scope` | `key` |
| a signed link's life | `signed_url_ttl_seconds` | `storage_access_signed_url_ttl_seconds` | `signed_url_ttl_seconds` | 3600 |
| keep outputs | `keep_outputs` | `storage_access_keep_outputs` | `keep_outputs` | `true` |
| delete kept files after (days) | `retention_days` | `storage_access_retention_days` | `retention_days` | 0 (never) |
| who signs a signed link | `signed_links` | `storage_access_signed_links` | `signed_links` | `store` |

**Gateway-wide, at runtime,** on the dashboard (Gateway → Settings → Stored
files) or through the API, as the gateway admin:

```sh
curl -X PUT http://localhost:4000/v1/admin/settings \
  -H "Authorization: Bearer $MASTER_KEY" -H 'content-type: application/json' \
  -d '{"storage_access_mode": "signed", "storage_access_signed_url_ttl_seconds": 900}'
```

`PUT /v1/admin/settings` replaces every gateway setting, so send the others
you keep along with it. `effective_storage_access` in the answer shows what
applies to apps with no setting of their own.

**Per app,** on the dashboard (Organization → Access to stored files) or
through the API, with the storage permission (org owner or admin):

```sh
curl -X PUT http://localhost:4000/v1/apps/$APP_ID/access \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"mode": "private", "owner_scope": "app"}'
```

```json
{
  "app_id": "…",
  "overrides": { "mode": "private", "owner_scope": "app", "signed_url_ttl_seconds": null },
  "effective": { "mode": "private", "owner_scope": "app", "signed_url_ttl_seconds": 900 },
  "updated_at": "2026-11-02T16:00:00Z"
}
```

- **A field you leave out or set to `null`** follows the gateway; `{}`
  clears them all.
- **Bad values** answer 400: `invalid_mode`, `invalid_owner_scope`,
  `invalid_ttl`, `invalid_keep_outputs`, `invalid_retention` or
  `invalid_signed_links`.
- **Every change** is in the org's audit log as `app.access.update`.

## Retention and deleting

- **`retention_days`** deletes an app's kept files that many days after they
  were made; 0 (the default) keeps them until they are deleted. An app's own
  value, 0 included, wins over the gateway's.
  - Every replica sweeps hourly, deleting each file from the store that holds
    it: the app's bucket or litegen's. A sweep has a pass for each app with a
    retention of its own, and one with the gateway's for every other app. The
    passes take turns, 500 generations at a time, and a pass goes on while it
    deletes, for up to 10 minutes in all, so a backlog is worked off in one
    sweep. A pass whose 500 delete nothing (nothing due, or only deletes that
    fail) sits out the rest of the sweep: an app whose bucket refuses costs
    one batch a sweep, however long the other passes go on.
  - A row is marked only once its files are gone. A delete that fails puts its
    row behind the rows not yet tried, so a bucket that keeps refusing doesn't
    hold up the others: the row is tried again at the end of its pass if that
    pass is still deleting, or else by a later sweep.
  - The record stays. `files_deleted_at` and `files_deleted_reason`
    (`retention`) appear on the generation, and the file route answers 410
    `files_deleted`.
  - litegen's local disk store also keeps its own cap,
    `image_storage.retention_days` (at least 7 days), whatever an app sets.
    For a longer retention, raise it, or use a bucket.
- **`DELETE /v1/generations/{id}`** deletes a finished generation's kept files
  and its request (the prompt and params, on the row and in the request log),
  and keeps its billing record. It answers the generation with
  `files_deleted_at` (`files_deleted_reason: deleted`).
  - A job still running is a 409 `generation_running`: cancel it first.
  - It needs `generation:delete:own|any` for a dashboard session; an API key
    must pass the app's owner scope.
  - Files in an app's own bucket are deleted too. A vendor's own file
    (`kept: false`) is the vendor's to delete.
- **Deleting an app or an org** deletes its files in litegen's own store, in
  the background. Files in the app's own bucket are yours, so they stay.

## Revoking links

- **`POST /v1/apps/{app_id}/access/revoke-links`** (storage permission)
  refuses every link litegen signed to the app's files before now, on every
  replica, at once.
- **`POST /v1/generations/{id}/revoke-links`** does the same for one
  generation (`generation:delete:own|any`, or the owner scope for a key).
- **`POST /v1/admin/revoke-links`** does it gateway-wide (the gateway admin).

Each answers `{ "revoked_at": … }` and is in the audit log. A revoked link
answers 403 `link_revoked`. Read the generation again for a fresh link.
`GET /v1/apps/{id}/access` shows `links_revoked_at`, the latest of the app's
and the gateway's.

**What can't be revoked:** a bucket's own presigned link (`signed_links:
store` on a bucket) works until it expires. Set `signed_links: litegen` to
make every signed link revocable, or keep the TTL short. Deleting a
generation deletes its files, which ends every link to them.

## Your app's own bucket

An app's bucket (`PUT /v1/apps/{app_id}/storage`, Organization → Storage on
the dashboard) holds its files instead of litegen's store. litegen reaches it
in one of two ways:

- **`credential_mode: access_key`** (default): an access key pair, stored
  encrypted.
- **`credential_mode: assume_role`** with `role_arn`: a cross-account IAM role
  in your AWS account, which litegen assumes with an external ID it assigns
  to the app. This is AWS's pattern for a third party.
  - `GET /v1/apps/{app_id}/storage` shows the app's `external_id`, even before
    a role is saved, and `litegen_principal_arn`, the principal to trust.
  - In the role's trust policy, allow `sts:AssumeRole` for that principal on
    the condition `sts:ExternalId` equals the app's external ID.
  - Grant the role `s3:PutObject`, `s3:GetObject` and `s3:DeleteObject` on
    the bucket.
  - litegen assumes the role when you save, and answers 400
    `assume_role_failed` with STS's reason if it can't. A role ARN that isn't
    one is a 400 `invalid_role_arn`.
  - Sessions last an hour and are renewed five minutes before they end. A
    presigned link can't outlive the session that signed it, so a signed link
    longer than the session left is litegen's own.
  - **litegen's own AWS identity** comes from `aws` in `litegen.yaml`
    (`access_key_id`, `secret_access_key`, `session_token`, `region`,
    `sts_endpoint`, and `principal_arn` to show customers), or else the
    standard chain (environment, profile, instance or container metadata,
    web identity).
  - **The external ID is derived from `LITEGEN__SECRETS_KEY`,** so rotating
    that key changes every app's external ID. Update the trust policies when
    you do.
- **`fallback_to_litegen_store`** (default `true`) says what happens while the
  bucket can't be reached (a revoked key, a role that no longer trusts
  litegen):
  - `true`: litegen's own store keeps the files meanwhile.
  - `false`: nothing reaches litegen's store. An image's history copy is
    skipped (the caller still has the image), and a video or 3D job waits for
    the bucket: retried, then failed at the 2-hour maximum age, refunded and
    webhooked.

## What changes in the answers

URLs are turned into the app's mode **when litegen answers**, not when it
stores. Rows keep the stored URL; signed links are fresh on every read. A
change of mode therefore applies to files made before it. Every place a
stored URL leaves litegen follows the mode:

- `POST /v1/images/generations`: `data[].url`, when images go to an S3 image
  store. Inline `b64_json` images are the caller's already.
- `GET /v1/videos/{id}`: `video_url`.
- `POST /v1/models3d/generations` and `GET /v1/models3d/{id}`: `assets[].url`.
- `GET /v1/generations` and `GET /v1/generations/{id}`: `result_url` and
  `metadata.assets[].url`.
- `GET /v1/generations/{id}/assets`: `data[].url`.
- `GET /v1/logs/{id}/artifact`: `output_value`. Under `signed` or
  `private`, an image the log keeps inline (base64) goes out only inside the
  owner scope: a key outside it gets `output_kind: "url"` with the file route
  (which refuses it), or no value when nothing was kept. Under `public`, any
  key that may read the log gets the image, as before.
- **Webhooks:** the key's own webhook gets the links that key may use.

Every signed link comes with **when it stops working**: `expires_at` beside
each asset's `url`, and `video_url_expires_at` beside a video's. Read the
generation again for a fresh link.

## Keep the bucket private

`signed` and `private` protect nothing if the bucket itself serves files to
anyone. On your bucket:

- **Turn on S3 Block Public Access,** or its equivalent on R2, Spaces or
  MinIO. litegen's keys still read and write through the API.
- **Signed links go to the bucket's own endpoint,** never to
  `custom_public_url`: a CDN in front of the bucket can't check an S3
  signature.
- **litegen's local store enforces the mode itself.** In `signed` mode its
  asset route serves only valid, unexpired links. In `private` mode it
  serves nothing, and the file route is the only way in.

## Per-customer rules: the `file_access` hook

When your app serves many customers through one litegen key, the key is no
boundary between them. A `file_access` hook is. It runs on the file route
after litegen's owner check, sees the file (and the end user it was made
for) and the caller, and can refuse. It can never open a file litegen would
refuse. It runs too when a request names a stored generation as an input
(`reference_images`, `source_video` or `source_model` of type `generation`),
for the file the job would read, so a customer can't feed another
customer's file into their own job ([docs/hooks.md](hooks.md)).

Name the end user on every generation request with `x-litegen-end-user`. A
custom-auth `pre_call` hook can set it for you. Then compare on the way out:

<!-- example: examples/hooks/file_access_end_user.py -->
```python
"""Each of your customers opens only the files made for them.

litegen records who a generation was for (`x-litegen-end-user`, set here by
custom_auth.py) and passes it to `file_access` as file["created_by"]["end_user"];
whoever is asking now is caller["end_user"]. The hook runs on litegen's file
route, after litegen's own owner check, so it can only refuse. A file made for
no one in particular (no end user recorded) is left to that owner check.

    storage_access:
      mode: private
    hooks:
      - path: hooks/custom_auth.py
      - path: hooks/file_access_end_user.py
"""


def file_access(request, file, caller):
    made_for = file["created_by"].get("end_user")
    if made_for is not None and made_for != caller.get("end_user"):
        return False  # 403
    return None
```

[docs/hooks.md](hooks.md) has the whole contract: return nothing or `true`
to serve; `false`, a string or a raised status to refuse. A failing hook
refuses with 503 unless `on_error: allow`. The same example in Node is
there too.

A signed link is handed to any caller in the owner scope without asking the
hook, so per-customer rules need `private` mode. Browsers fetch private
files with credentials: a session cookie, or your custom auth's cookie
through `pre_call` (see the cookie example in docs/hooks.md).

## Good to know

- **Links already handed out:**
  - A bucket's own presigned link can't be revoked; it works until it
    expires. litegen's own links can be (see [Revoking links](#revoking-links)).
  - litegen's local store's signed links stop the moment the app goes `private`.
  - A CDN that cached a `public` file keeps serving it. litegen marks public
    files cacheable for a year.
- **litegen's own signed links work on every replica and survive a restart.**
  With `LITEGEN__SECRETS_KEY` set, links are signed with a key made from it, and
  that key wins. (A value that isn't base64 for exactly 32 bytes is ignored for
  links, with a startup warning.) Without it, litegen generates one key the
  first time it starts and stores it in the database's `server_secrets` table.
  Every replica reads that key back. Anyone who can read the database can then
  forge file links, but database access already exposes the files' metadata and
  URLs. If the database answers with an error when a replica reads the key at
  startup, litegen tries again for about 8 seconds. If it still can't read the
  key, or the stored value is damaged, the replica refuses to start and says
  why: a key of its own would sign links that no other replica, and no
  restart, accepts. Fix the database, or set `LITEGEN__SECRETS_KEY`. For a
  damaged value, delete the `link_signing_key` row of `server_secrets`; links
  already handed out then stop verifying.
- **Idempotent replays** (`Idempotency-Key`) return the first answer, links
  and all. Read the generation for fresh links.
- **The dashboard** shows a private app's files through its own session, so
  previews keep working in every mode.
- **A finished video is kept once, and never held in memory.** The client's
  poll and the poller (on any replica) claim the right to keep it before any of
  it is downloaded. A poll without the claim answers `processing` once more,
  and the next one reads the kept video. The keeper streams a video named by a
  link to a spool file, hashing it as it arrives, reads its length, frame rate
  and size from the file, and hands the file to the store: a rename for the
  local store, one presigned PUT with its length for a bucket.
  - **Where it waits:** `<local_dir>/litegen/.spool`, in S3 mode too (in the
    Docker image, the `/app/data` volume). litegen checks at startup that it can
    write there; if it can't, it logs a warning and uses the system temp dir
    (`TMPDIR`). A video in flight takes disk, up to 1 GiB each, never RAM. A
    spool file is removed when its video is kept or fails; what a crash leaves
    is removed at the next start once it is a day old. A failure to write the
    spool (a full disk) is passing: the next poll tries again.
  - **How many at once:** each replica keeps up to 8 videos at a time; the rest
    wait their turn. Veo, Bedrock and Sora hand a video over whole (up to 1 GiB
    each, Sora now capped like the others), so up to 8 such videos can be in
    memory at once on a replica, once each.
  - **Into a bucket:** a kept video goes in with one presigned PUT through
    litegen's own HTTP client, whether a link named it or the vendor handed
    it over as bytes (Veo, Bedrock, Sora), into the operator's bucket and a
    tenant's alike; so does every other file litegen keeps there (3D files,
    late images). Such an upload may take up to an hour while it keeps
    moving. One that stops moving for 60 s (the bucket takes nothing more,
    or hasn't answered 60 s after the last byte) is cut off, and the next
    poll tries again. Only an image a generation call answers with still
    goes into the operator's own bucket through rust-s3, which gives the
    whole upload 60 s.
  - **The claim** lasts 15 minutes. It is renewed as the keep starts (Veo,
    Bedrock and Sora are claimed before their download, which may take up
    to 10 minutes) and while a large upload runs.

## Not covered yet

- A vendor link that needs the vendor's key (Google Veo's) can be neither kept
  nor handed out, so no such model is carried.
- Retention per end user, and object lock or legal hold.
- In `signed` mode, a GIF video behind litegen's signed link previews as a
  video on the dashboard (only the mock models make GIF video).
