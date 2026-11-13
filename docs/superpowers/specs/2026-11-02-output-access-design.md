# Stored outputs: access modes, ownership and the file hook — design

Status: the operator delegated the decisions ("research best practices and
the most needed use cases and build it the best way, with flexibility to
customize it per use case"), 2026-11-02. Every call below is mine, made from
the research cited in "Grounds".

## Intent

Every output litegen keeps — images, videos, 3D meshes and their derived
formats — lives in the customer's own bucket (per app, set on the dashboard)
or the gateway's store. Today every one of them is handed out as a **plain
public link with no check on who asks**. This design makes **who may open a
stored file** a setting: per app on the dashboard or the API, with defaults
in `litegen.yaml`, in the same YAML-then-database layering LiteLLM uses. A
hook covers what the settings can't.

## Grounds (research, 2026-11-02)

- **Keep buckets private; authorize every object fetch server-side;
  unguessable keys are not authorization** (OWASP IDOR Prevention Cheat
  Sheet; S3 presigned-URL guidance: private ACL by default, short expiries,
  server-chosen keys).
- **Short-lived links are the industry norm for generated media:** OpenAI's
  image URLs are valid 60 minutes; Replicate deletes API outputs after an
  hour; fal signs links for up to 7 days.
- **Ownership by creator, widening by group:** LiteLLM's managed files belong
  to the key/user/team that created them; a key with a team reaches the
  team's objects, a bare key only its own; non-owners get 403.
- **BYO buckets:** least privilege, Block Public Access on, cross-account
  roles with an external ID preferred over long-lived keys (Datadog, AWS
  APN). Roles are a follow-up here (see Non-goals); the existing per-app
  access keys stay.

## The use cases this serves

| use case | setting |
|---|---|
| A backend fetches results with its own API key | `private`, scope `key` |
| A web or mobile app shows results to its users | `signed` (links expire) |
| Public galleries, embeds, CDN | `public` (today), optionally `custom_public_url` |
| Several services of one customer share results | `private` or `signed`, scope `app` or `org` |
| Custom per-user rules (end users, plans, sharing) | `private` + the `file_access` hook |
| Compliance: data stays in the customer's bucket | the existing per-app bucket + `private`/`signed` |

## Decisions

### 1. Access modes, per app

| mode | the URL litegen hands out | who can open it |
|---|---|---|
| `public` | the stored URL (bucket, `custom_public_url`, or litegen's asset route), as today | anyone with the link |
| `signed` | a link that expires: an S3 presigned GET for a bucket, or a litegen-signed URL (HMAC over key and expiry) for litegen's local store | anyone with the link, until it expires (default 3600 s; 60 s – 7 days) |
| `private` | litegen's file route, `GET /v1/generations/{id}/assets/{index}` | only callers who pass the owner check (§2) and the `file_access` hook (§4); litegen streams the bytes, so the bucket can stay fully private |

- **Default `public`**, my call, for compatibility: existing consumers (aipix)
  fetch these URLs without credentials. The docs and `litegen.example.yaml`
  recommend `signed` for new deployments.
- **URLs are minted when litegen answers, not when it stores.** Rows keep
  the stored URL; every place a stored URL leaves litegen (the generation
  answers, video and 3D status, `GET /v1/generations…`, webhooks) passes it
  through one function that turns it into the app's mode. Signed links are
  fresh on every read.
- **Every asset entry gains `expires_at`** (RFC 3339, `signed` only), so a
  client knows when to re-read the generation.
- In `signed` and `private` mode litegen's own local asset route stops serving
  that app's files without a valid signature or credentials.

### 2. Owner scope, per app

Who passes the owner check for a `private` file:

| scope | who may open it |
|---|---|
| `key` (default) | the API key that made it |
| `app` | any key of the same app |
| `org` | any key of the same org |

- **Always also:** the master key; dashboard sessions with
  `generation:read:any` (admins); dashboard sessions with
  `generation:read:own` for files made by their own keys (litegen's existing
  rule).
- **No end-user isolation by default** (the operator's call): the key is the
  boundary. Per-user rules belong to the hook.
- Reading the generation's JSON keeps today's rule (same org); only the files
  follow the scope. In `signed` mode a caller outside the scope is answered
  the private route instead of a signed link, so it never holds a working
  link to a file the scope keeps from it.

### 3. End users are recorded

The `x-litegen-end-user` a request names (by itself, or injected by a
`pre_call` hook) is stored on the generation (`metadata.end_user`), so the
file hook can compare it.

### 4. The file route joins the hooks, plus `file_access`

- `GET /v1/generations/{id}/assets/{index}` joins the hooked routes:
  `pre_call` runs first (custom auth injects a key), `post_call` sees its
  headers (the file streams through).
- **`file_access(request, file, caller)`**, a new hook function in the same
  hook files, runs on every fetch through the file route, after the owner
  check passes. In `private` mode that is every fetch.
  `file` = `{generation_id, index, kind, format, size_bytes, media_type,
  model, created_at, created_by: {key_id, app_id, org_id, end_user}}`.
  - Return nothing (or `true`) to serve.
  - Return `false` (403), a string (403 with that message), or raise a
    status, to refuse.
  - **Narrowing only** (my call): it can refuse what litegen would serve,
    never serve what litegen refuses, so a buggy hook can't leak another
    tenant's files. Widening is the scope setting's job.
  - A signed link is minted for any caller in the scope without calling the
    hook. Per-user rules therefore need `private` mode.
- A failing `file_access` refuses (503) unless the hook's `on_error: allow`.

### 5. Settings: YAML defaults, per-app overrides

```yaml
storage_access:
  mode: public               # public | signed | private
  owner_scope: key           # key | app | org
  signed_url_ttl_seconds: 3600
```

- Per app: `PUT /v1/apps/{app_id}/access` with any of the three fields, and
  `null` to fall back to the YAML; `GET` returns the stored overrides and the
  effective values. Stored in a new `app_file_access` table (both
  databases), so an app without its own bucket can still choose its mode.
- **The dashboard's app storage panel** shows and edits them.
- The effective value is the app override, else the YAML default.
- Generations with no app use the YAML defaults.

## Non-goals (follow-ups, named)

- **Vendor-link passthrough** (keep nothing): vendor links expire within the
  hour (OpenAI, Replicate) and converted 3D formats have no vendor link, so
  it needs its own design.
- **Cross-account IAM roles with an external ID** for BYO buckets, replacing
  stored access keys.
- **Cache and other gateway settings in the database.**
- **Retention policies** per app (the local store already sweeps by
  `retention_days`).
- **Revoking a signed link early** (S3 presigned links can only expire).

## Anti-goals

- **Changing what existing clients get by default.** `public` stays the
  default, so a deployment with no settings answers exactly as today.
- **A file reachable in `private` mode without the owner check.** That
  includes litegen's own asset route and a guessed key.
- **A signed link that outlives its expiry,** or that can be forged or edited
  into another file's link.
- **Hook code that can widen access.**

## Constraints

- **OpenAPI.** It changes: `expires_at` on asset entries, the
  `/v1/apps/{app_id}/access` routes, and the `file_access` hook in the docs.
  Ping the peer session before editing `openapi.rs`.
- **The HMAC key** for litegen-signed links comes from `LITEGEN__SECRETS_KEY`
  when set; otherwise from a key generated at boot (links don't survive a
  restart, which is noted in the docs).
- **The mesh library isn't touched,** and nothing here couples to it.

## Proof

- **Unit tests:** signing and verification (expiry, tampering, another key's
  link), mode resolution (YAML vs app), presigned S3 link shape and expiry.
- **API tests, per mode:**
  - `public` is unchanged.
  - `private`:
    - scope `key`, `app` and `org` against other keys;
    - the master key and admins;
    - 404 vs 403;
    - the file hook narrowing, and failing;
    - custom-auth `pre_call` on the file route;
    - litegen's local asset route refuses.
  - `signed`: the link works without credentials until it expires, then
    doesn't; a tampered link fails; the URLs in webhooks follow the mode.
- **Settings:** YAML default vs app override vs `null` reset; `GET` shows
  effective values.
- **The dashboard:** a browser test sets the mode on the app panel, and a
  generation's file then follows it.
- **The guide** (`docs/storage-access.md`) quotes a file-hook example that the
  tests run.
