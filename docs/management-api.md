# Management API

LiteGen's management API covers what the LiteLLM proxy's management endpoints
do, mapped onto LiteGen's own model: orgs ≈ LiteLLM organizations, apps ≈ teams,
API keys ≈ virtual keys, and end users ≈ customers. Every endpoint is in
`/openapi.json`, in both SDKs, and used by the dashboard. Every endpoint is
also an MCP tool at `/admin/mcp`, LiteLLM's path for its admin MCP; see
[docs/mcp](mcp/README.md).

## Key scopes

An API key holds one or more of three scopes. They split the AI API from the
management API the way LiteLLM's key types do.

| Scope | Opens |
|---|---|
| `generate` | Generating images, video and 3D; cost quotes; job status; mesh conversion; the model catalog (`/v1/models`, `/v1/providers`, `/v1/utils/models3d/formats`) and the key's generations and their assets (`/v1/generations…`); cancelling, deleting and revoking the links of the generations its app's owner scope lets it change (by default, its own) |
| `read` | Reads only: the model catalog and generations too, plus request logs, stats, spend, limits and end users; in single-tenant mode, in the deployment's org, also provider health (`GET /health`, which checks the gateway's own vendor keys; in hosted mode only the master key reads it) |
| `admin` | Managing keys, budgets and limits, end users, the audit log and the cache; in single-tenant mode also the gateway's models, providers, routes and settings |

The Keys page offers them as presets:

| Preset | Scopes | LiteLLM key type |
|---|---|---|
| AI | `generate` | `llm_api` |
| Management | `read,admin` | `management` |
| AI + management | `generate,read,admin` | `default` |
| Read-only | `read` | `read_only` |

A key created without `scopes` gets `generate,read`. A key never gets a scope
its creator could not grant: only owners and admins, the master key, or an
`admin`-scope key may grant `admin`. That check runs when a key is created and when its scopes
are edited. An unknown scope is a 400.

## Key webhooks

A key's `webhook_url` gets a signed `POST` when one of its video or 3D jobs
ends. `PATCH /v1/keys/{id}` with `"webhook_url": null` (or `""`) removes it;
the key then shows `webhook_url: null`. Where it may point:

- **Hosted mode:** public addresses only, for every key.
- **Single-tenant mode:** the deployment's own network (private, loopback and
  link-local addresses) is open only to the deployment's own keys: keys made
  with the master key (no owner), and keys whose owner is an active Owner or
  Admin of the deployment's org. Any other key, a Member's for instance, or
  a key of a side org whose owner is not an Owner or Admin of the deployment's
  org, is held to public addresses. The rule follows the key's owner, whoever
  edits the key, and is checked when the URL is saved (400
  `invalid_webhook_url`), at `POST /v1/keys/{id}/test-webhook`, and at every
  delivery: a demoted or departed Admin's key stops reaching inside at once.
  Set `webhooks.internal_targets: everyone` (`LITEGEN__WEBHOOKS__INTERNAL_TARGETS=everyone`)
  to let any key's webhook reach inside, as before.
- **At startup**, a single-tenant deployment names in its log each active key
  whose stored webhook now gets no deliveries: a `SECURITY` warning per key,
  with the key's id and prefix, its owner and the URL's host (never the rest
  of the URL), then one that counts them (`refused_keys`). It uses the check
  every delivery makes, so nothing is logged with `internal_targets:
  everyone` or when no key is affected. The check runs in the background and
  waits at most 5 seconds for each host's name, so startup never waits for
  it, and a host whose name doesn't resolve then isn't named.

A delivery keeps the first 16 KiB of the endpoint's reply
(`GET /v1/keys/{id}/webhook-deliveries`).

**Retries.** A delivery is done at the first 2xx answer. A `410 Gone` ends it
at once: the endpoint says it is gone for good. Anything else is retried: a
network error, no answer within 30 seconds, or any other status, a 404 too.
litegen waits 10 seconds, 30 seconds, then 1, 2, 5, 10 and 30 minutes, then 1,
2, 4, 8 and 8 hours between attempts. That makes 13 attempts, the last about
24 hours after the first. After that the delivery is failed, and nothing sends
it again. Each attempt is a row of `GET /v1/keys/{id}/webhook-deliveries`, and
its `delivery_id` names its delivery. A delivery that can't be sent shows there
too, as a row with no `status_code` and the reason in `error_message`: the
key's `webhook_url` was cleared, or it points where the key may no longer
send. Fix that, then send it again by its `delivery_id` (below).

**Each attempt is built when it is sent**, from the generation and its key as
they are then. Its file links are signed then (in a `signed` app they work for
the app's `signed_url_ttl_seconds` from that attempt), it goes to the key's
`webhook_url` of that moment, and the rules above are checked again. A key
whose `webhook_url` was cleared gets no more attempts. The delivery keeps the
payload of its latest attempt once, and the list shows it on each of its
attempts. The first attempt goes once the job is settled, so a cancelled
job's webhook shows its refund (`cost_usd: 0`).

**Sending one again.** `POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver`
makes a delivery that has ended, `delivered` or `failed`, due again: a fresh
series of attempts under the same delivery id, numbered from 1 again in the
list, with the whole retry schedule. A receiver that still holds that id
from handling it treats it as a repeat. It answers `202` with the delivery's
state (`WebhookDeliveryState`), and a litegen process sends the first attempt
within about a second, to the key's `webhook_url` of that moment and under
the rules above, checked again then. It is refused with `409
delivery_pending` while the delivery is still being sent, with `429
redeliver_too_soon` (and `Retry-After`) within a minute of its last
redeliver, and with `400` when the key has no `webhook_url` or one it may no
longer use. Who may: an `admin`-scope key or the master key, in the key's
org; an Owner or Admin for any key of the org; a Member for a key they own
(`key:test_webhook:own`). A Viewer may list their key's deliveries but not
send one again.

**Keeping the log.** litegen keeps the webhook log until you set a number:
`webhooks.log_retention_days` (`LITEGEN__WEBHOOKS__LOG_RETENTION_DAYS`, 0 to
36500) is 0 unless set, which keeps every delivery and attempt row for good.
Once it is set, the hourly retention sweep deletes a delivery that ended, and
each attempt row, that many days after it ended or was made, on the
database's clock. A delivery still being sent is never deleted.

**Duplicates.** Every request carries `X-Litegen-Delivery-Id`: `whd-` and a
UUID. It is the same on every attempt of one delivery, and new for the next
delivery. Delivery is at least once: an endpoint can get one delivery twice,
for instance when its 2xx is lost or comes after 30 seconds, or when the
litegen process sending it stops mid-attempt and another sends it again. So
keep the ids it has handled for a day, and answer a repeat with a 2xx without
acting on it again. The signature covers the body, not this header. Each
`POST /v1/keys/{id}/test-webhook` sends one attempt with an id of its own,
signed like a job's webhook.

**What a restart loses.** Nothing. The job's terminal write saves its webhook
as a delivery in the database, in the same statement, and every litegen
process sends the deliveries that are due. The process that ended the job
sends the first attempt; should it die first, another sends it 30 seconds
after the job ended. A crash, a kill or a restart only delays them: another
process, or the same one once it is back, takes a dead one's deliveries over
within a minute, and one waiting for its next attempt keeps its time. A
graceful stop lets the attempts in flight finish within its grace
(`server.shutdown_grace_secs`) and leaves the rest due; an attempt still
sending when the grace ends is sent again by another process once its claim
lapses (60 seconds), never while it may still be under way. The job's outcome is
also at `GET /v1/generations/{id}` (or `GET /v1/videos/{id}`,
`GET /v1/models3d/{id}`).

## Limits and budgets

Each principal can have one **limits** row: a key, an org, an app, an org
member, or an end user.

| Field | Meaning |
|---|---|
| `max_budget_usd` | Hard cap for the current window. A request that would exceed it gets **402 `budget_exceeded`**. |
| `soft_budget_usd` | Alert threshold. Crossing it writes a `budget.soft_limit_reached` audit entry and POSTs to the gateway's `alert_webhook_url`, once per window. |
| `budget_duration` | Window length: `30s`, `30m`, `12h`, `30d`, `1w`, `1mo`. Leave it out for a lifetime cap. Windows roll forward from when the budget was set. |
| `allowed_models` | Model allowlist. Any other model gets **403 `model_not_allowed`**. |
| `rpm_limit` | Requests per minute. Over it: **429** with `Retry-After`. |
| `max_parallel_requests` | Cap on requests in flight. Over it: **429 `parallel_limit_exceeded`**. |
| `metadata` | Free-form JSON object. |

A generation is checked against every row that applies to it: its key, its
org, its app, the member who owns the key or session, and the end user. The
strictest limit wins. Before dispatch the estimated cost is reserved on every
row; after dispatch each row is settled to the actual cost. If any row refuses,
the reservations already taken are rolled back. The key's older `token_quota`
(a lifetime USD cap) still applies as well. A row's `spend_usd` includes what
requests in flight hold. A request whose replica crashed holds its
reservations for about two minutes more, until any replica's sweep gives them
back ([scaling.md](scaling.md#several-replicas), "Reservations"). Spend is
counted in the window in which the request was reserved. A refund or give-back
that arrives after the window rolled comes off the new window's spend, never
below zero. An example is a video job charged just before the roll that fails
just after it. The new window can therefore undercount by at most the prices
of the requests in flight at the roll.

The org's allowed-model pool and the app's model access
(`/v1/orgs/{id}/allowed-models`, `/v1/apps/{id}/allowed-models`) are now
enforced at generation time. `GET /v1/models` lists only the models the caller
may use; `GET /v1/models?all=true` lists every model the gateway serves (the
dashboard's Providers page edits the pool from it, so a model left out of the
pool stays listed to be added back).

| Endpoint | |
|---|---|
| `GET /v1/limits?entity_type=` | Every limits row in the caller's org |
| `GET /v1/limits/me` | The rows that apply to the caller (LiteLLM `/key/info`) |
| `GET/PUT/DELETE /v1/keys/{id}/limits`, `POST …/reset` | Key limits; `reset` zeroes the window's spend |
| `POST /v1/keys/{id}/block`, `/unblock` | Block or unblock a key |
| `GET/PUT/DELETE /v1/orgs/{id}/limits`, `POST …/reset` | Org budget |
| `GET/PUT/DELETE /v1/apps/{app_id}/limits`, `POST …/reset` | App (team) budget |
| `GET/PUT/DELETE /v1/orgs/{id}/members/{user_id}/limits`, `POST …/reset` | Member budget: covers the member's session requests and every key they own |

## End users

Send `x-litegen-end-user: <id>` with a generation to attribute it to one of
your customers. An id is registered the first time it is seen. A blocked end
user gets **403 `end_user_blocked`**. `x-litegen-tags: a,b` tags a request for
spend reporting.

| Endpoint | |
|---|---|
| `GET/POST /v1/end-users` | List or create (optionally with `limits`) |
| `GET/PATCH/DELETE /v1/end-users/{id}` | Read, update alias/blocked/metadata, delete |
| `POST /v1/end-users/{id}/block`, `/unblock` | Blocking an id never seen before registers it as blocked |
| `GET/PUT/DELETE /v1/end-users/{id}/limits`, `POST …/reset` | Per-end-user budget |

## Spend

| Endpoint | |
|---|---|
| `GET /v1/spend/report?group_by=` | Spend and request counts grouped by `day`, `model`, `provider`, `media_type`, `status`, `org`, `app`, `key`, `user`, `end_user` or `tag` |
| `GET /v1/spend/logs` | The attributed request log, paged |

Both take `from`/`to` (RFC 3339 or `YYYY-MM-DD`) and filters on `app_id`,
`key_id`, `user_id`, `end_user`, `model`, `provider` and `tag`. Results are
scoped to the caller's org, and an app-bound API key only sees its own app. The
platform admin (the hosted master key) sees every org, or one org via `org_id`.

A request's log row, and its artifact (`GET /v1/logs/{id}/artifact`), are
written before it answers, on a task a caller who hangs up can't cut: a
process that stops or dies after answering keeps the record of every request
it billed. An image answered from the cache has a row of its own too, at what
it was charged (the cached answer's price, or nothing with `cache_charge_hits:
false`), and its `cache_hit` is true, in `GET /v1/logs` and `GET
/v1/spend/logs` alike; on every other row it is false ([gateway
settings](gateway-settings.md)). Its artifact names the generation it was
served (`output_kind: "generation"`, the id in `output_value`) instead of
holding a copy of the image, and `DELETE /v1/generations/{id}` of that
generation deletes it too.

**Keeping the request log.** litegen keeps it for good unless `logs` in
`litegen.yaml` says otherwise. The hourly retention sweep then deletes, on
the database's clock:

- a request's log row, with its spend tags, `logs.retention_days` after the
  request (`LITEGEN__LOGS__RETENTION_DAYS`);
- its artifact (prompt, params and output, up to 2 MiB) after
  `logs.artifact_retention_days` (`LITEGEN__LOGS__ARTIFACT_RETENTION_DAYS`),
  or else after `logs.retention_days`. `GET /v1/logs/{id}/artifact` then
  answers 404, while the row can stay.

Both are 0 unless set, which keeps everything; each takes 0 to 36500 days,
and litegen refuses to start with artifact days above a nonzero
`retention_days`. Spend reports and the request log read only the rows
kept: `GET /v1/spend/report`, `GET /v1/spend/logs`, `GET /v1/logs` and `GET
/v1/stats` (the dashboard's Overview, Logs and Spend pages) cover the last
`retention_days` at most, whatever `from` asks for. Budgets don't depend on
the log (each keeps its own running spend), and generations
(`GET /v1/generations/{id}`) are never deleted with it ([runbook](RUNBOOK.md#database-growing-what-grows-and-how-to-bound-it)).

## Gateway admin

These endpoints are for the platform admin only, plus owners/admins and
`admin`-scope keys in single-tenant mode. Changes apply to the next request and
are stored, then re-applied at boot. Settings and routes reach every replica
within 15 s.

| Endpoint | LiteLLM | |
|---|---|---|
| `GET/POST /v1/admin/models` | `/model/info`, `/model/new` | Add a model at runtime. The body is a `models/*.yaml` row and goes through the same validation. The vendor model called is the part of the id after `<provider>/`. |
| `GET/PUT/PATCH/DELETE /v1/admin/models/{id}` | `/model/update`, `/model/block`, `/model/delete` | `PATCH {disabled}` hides any model, catalog or runtime. Catalog models are otherwise read-only. |
| `GET /v1/admin/providers`, `GET/PUT/PATCH/DELETE /v1/admin/providers/{name}` | `/credentials` | Set keys, `api_base`, `model_mapping`, headers, options or `enabled` ([provider-config.md](provider-config.md)); a header litegen can't send is a 400 `invalid_provider_config`. `PUT` replaces the settings; `PATCH` changes only the fields it sends, `null` clearing one, and keeps the rest (the dashboard's Configure form saves this way). A key left out keeps its stored value; so does one sent blank in a `PUT` or `null` in a `PATCH`, while a `PATCH` that sends it as `""` removes it (the env/YAML key applies again). Keys are AES-GCM encrypted at rest and only shown as `…last4`. Extra header values are shown by name only; with `encrypt_provider_credentials` on they are encrypted at rest too, with the user info of URLs ([provider-config.md](provider-config.md#credentials-at-rest)). Unset fields fall back to the env/YAML config. In-flight jobs keep the instance they started with. |
| `GET/PUT/DELETE /v1/admin/routes` | router settings, fallbacks | Model routes: fallback, weighted round robin, lowest cost, lowest latency, caching |
| `GET/PUT /v1/admin/settings` | `/config/update`, the UI's caching and router settings | `cost_markup_percent`, `alert_webhook_url`, a default end-user budget, stored-file access ([docs/storage-access.md](storage-access.md)), the cache, retries, vendor timeout and circuit breaker ([docs/gateway-settings.md](gateway-settings.md)), and `encrypt_provider_credentials`, which encrypts the credentials in runtime provider configs at rest; each save stores every config again as it says, and unlike the other fields a save that leaves it out keeps it ([provider-config.md](provider-config.md#credentials-at-rest)). The answer's `limits` gives each numeric setting's allowed range |

The audit log never keeps a secret these endpoints take. A provider change's
entry keeps each extra header's name, with `[redacted]` for its value. A
settings change's entry shows `[redacted]` for `alert_webhook_url`, since a
webhook URL can carry its secret in the path or in the host. Only
`GET /v1/admin/settings` shows the whole URL. An `api_base`'s user info
(`https://user:pass@host`), and that of a URL in `options`, shows as
`[redacted]` in the provider view and in the entry. A settings save that
switches `encrypt_provider_credentials`, or stores configs again, adds a
`provider.encrypt_credentials` or `provider.decrypt_credentials` entry with
how many configs it stored again and their names.

Runtime models are refused for `fal`, whose adapter maps model ids through a
fixed endpoint table. Add fal models to `models/fal.yaml` instead.

## Invitations

LiteGen sends no email. The inviter hands the invitation over:

| Endpoint | |
|---|---|
| `POST /v1/orgs/{id}/members` | Invite to that org (`member:invite`). `POST /v1/users` does the same for the org the caller acts in (`invitation:send`). |
| `GET /v1/orgs/{id}/invitations` | The org's pending invitations, newest first, without their tokens (`member:invite`) |
| `DELETE /v1/orgs/{id}/invitations/{invitation_id}` | Revoke one: its token accepts nothing after (`invitation:revoke`). A 409 `invitation_accepted` for one already accepted (remove the member instead), a 404 for one that isn't the org's |

Owners and Admins hold those permissions, and the master key passes them all
(the hosted master key acts in no org, so it invites through the org's path).
API keys can't invite. The invited role must be below the inviter's own role
in that org: an Admin invites Members and Viewers, an Owner Admins as well.
Anything else is a 403 (`forbidden` on the org's path, `forbidden_permission`
on `POST /v1/users`). The master key may invite to any role but Owner. The answer to an invitation carries its `token` and
`invite_url`, the
dashboard's page that accepts it (`/invite/{token}` on the origin of
`server.public_base_url`; null when that isn't set, so build it from `token`
on the dashboard's origin). Give one of them to the invitee, who accepts at
that page or with `POST /v1/auth/invitations/{token}/accept`. The token
accepts once, for 7 days, and no other answer shows it (litegen keeps only
its hash): for a lost one, revoke the invitation and invite again. Accepting
uses the invitation up in the same transaction that makes the account and the
membership, so an acceptance and a revocation at the same moment end one way,
never both. Each invitation and revocation is in the org's audit log
(`invitation.create`, `invitation.revoke`). That covers a revoke by hand, and
those that come with a change, which carry a `reason`: a removal
(`member_removed`), a role change to one that can't invite
(`inviter_role_changed`), a deactivation (`inviter_deactivated`) and a
confirmed reset (`inviter_reset`). An invitation is stored only while the
sign-in sending it (a session, or an app's OAuth grant) is still live,
checked in the statement that stores it: one whose request was still running
when the sender's reset or deactivation committed answers
`401 sign_in_ended`, and nothing is stored.

The invitee's email must be an address (one `@`, a name before it, a
domain with a dot after it, no spaces, control or invisible characters
(format and default-ignorable ones included), no unassigned code points, no
look-alike letters that NFKC normalization would change, at most 254
characters); anything else is a 400 `invalid_email`, as at signup. Letters
from another script mixed in (a Cyrillic `а` among Latin ones) aren't
detected.
`LITEGEN__DEV__EXPOSE_INVITE_TOKENS`, which versions before 2026-11-15
needed to show the token, still adds `_dev_token`, the same value. It is
deprecated: read `token`.

## Password reset links

`POST /v1/users/{id}/password-reset` issues a reset link for an account, as
an invitation does for a new one: the answer carries `token` and `reset_url`
(the dashboard's `/reset-password/{token}` page on the origin of
`server.public_base_url`, or null), shown only once. The token sets a new
password once, within the hour, with `POST /v1/auth/password-reset/confirm`.
A link takes the whole account over, so only the master key may issue one
for any account (and so for the deployment's Owner). A signed-in person may
issue one only for an account they outrank in every org it belongs to, as an
Owner or Admin there (`member:write`): an Admin for Members and Viewers, an
Owner for Admins too. An app signed in with OAuth may do so only inside its
grant's org. No one issues one for their own account: that is a 403 pointing
to `PATCH /v1/account`, which asks for the current password. For a
deactivated account, the issuer must also be one who may reactivate it: in
single-tenant mode a person who outranks it in the deployment's org, and in
either mode the master key (a 403 otherwise). A new link voids the account's
older unused ones. So does any change to the account's roles, ownership or
memberships, and deactivating it (migration 042's triggers). litegen keeps
only each token's hash. A link is stored only while the issuer's sign-in is
still live, checked in the statement that stores it: one whose request was
still running when the issuer's reset or deactivation committed answers
`401 sign_in_ended`, and nothing is stored (nor any older link voided).

Confirming does all of this in one transaction: it uses the token up,
checks that the link's issuer may still issue it, writes the password,
reactivates a deactivated account, ends the account's sessions, revokes its
apps' OAuth sign-ins (MCP clients) and their unredeemed codes, and moves the
account's sign-in epoch on. If any step fails, nothing changes, the answer is
a 500, and the link still works. The issuer check asks what the issue did,
again, now: the issuer still active, still above the account in every org it
belongs to (`member:write` there), and for a deactivated account still
allowed to reactivate it. If not, the answer is a 400 `invalid_token` ("The
link's issuer may no longer issue it") and nothing changes. The master key's
links have no issuer and always qualify. An issuer's unused links are also
deleted when the issuer is deactivated, removed from an org or has a role
changed (043's triggers).

Sessions, OAuth approvals and OAuth grants record the account's sign-in epoch
they began under (a login reads it before checking the password). A reset, a
deactivation or a change of one's own password moves it on, so a sign-in in
flight while one commits is refused when it lands: its session doesn't
authenticate, its code doesn't redeem, and its grant's tokens and refresh
stop working. Reactivating an account moves its epoch on too.
`PATCH /v1/account` (changing your own password) ends your other sessions,
your apps' OAuth sign-ins and their codes, and deletes the unused reset links
you issued, in the password write's transaction, and keeps the session you
sent it from. It writes only while your account still holds the password it
checked, is active, and is at the epoch of the session sending it; otherwise
it answers `401 sign_in_ended` ("Your sign-in ended while this request ran (a
reset, a deactivation or another password change): sign in again") with
nothing written. It keeps the pending invitations you sent, so a routine
change doesn't void real ones; `GET /v1/orgs/{id}/invitations` lists them
and `DELETE /v1/orgs/{id}/invitations/{invitation_id}` revokes one (or the
Members page). A confirmed reset and a deactivation also delete the unused
links the account issued and revoke the pending invitations it sent, in their
own transactions, each with an `invitation.revoke` audit entry (`reason`
`inviter_reset` or `inviter_deactivated`). `GET /v1/account/sessions` lists only sessions of the
account's current epoch. A session's `id` there is the SHA-256 hash of its
token: litegen keeps only that hash, so a copy of the database holds no
session that works, and the `id` sent as the cookie doesn't sign in. Its API keys keep working: revoke them on the Keys page or with
`DELETE /v1/keys/{id}`. The audit log has each link as
`user.password_reset_link` and each confirmed reset as `user.password_reset`
(with `"reactivated": true` when it brought the account back), both under
each org the account belongs to. It isn't
an MCP tool: issue it from the dashboard or the API.

## Role changes, removal and deactivation

`PATCH /v1/orgs/{id}/members/{user_id}` (and, in single-tenant mode,
`PATCH /v1/users/{id}` with a `role`) needs a role above the member's current
one, and never grants one at or above the caller's own: an Owner moves people
among Admin, Member and Viewer, an Admin only between Member and Viewer, and
no one changes their own role. Anything else is a 403. The Owner's role moves
only with transfer-owner. The master key may make any other change. Each
change is in the audit log: `org.member.update` (target `membership`) or
`user.update`, with `role` and `previous_role`.

Removing a member (`DELETE /v1/orgs/{id}/members/{user_id}`), deactivating
an account (`PATCH /v1/users/{id}` with `is_active: false`, or
`DELETE /v1/users/{id}`) and reactivating one (`is_active: true`) follow the
same rule, since removing someone and inviting them back lower is a demotion
by another door. A person acts only
on someone they outrank in that org (in single-tenant mode, the deployment's
org); anything else is a 403 (`forbidden` for a removal,
`forbidden_permission` for a deactivation or a reactivation). Removing
yourself, to leave, stays allowed; deactivating yourself through these routes
doesn't. Deactivating ends the account's sessions, revokes its OAuth grants
(with their tokens) and unredeemed codes, deletes its unused reset links (and
the ones it issued), and revokes the pending invitations it sent, in every
org, in the transaction that deactivates it. An
account with no role in the deployment's org is outranked by anyone who holds
the permission. The master key may remove or deactivate anyone but the
Owner. In hosted mode ownership lives in each org's membership, so the
master key may deactivate any account, a tenant's signup included, and
`POST /v1/users/transfer-owner` answers `400 transfer_owner_per_org` (use
`POST /v1/orgs/{id}/transfer-owner`). Both are audited as before:
`org.member.remove`, and `user.update` or `user.delete`. Every check in `PATCH /v1/users/{id}` comes before its first
change, so a refused request changes nothing. In single-tenant mode, an
account with no role in the deployment's org still takes the role-change
rule, with its own account role as the current one.

## Audit log

`GET /v1/audit` lists the entries of the org the caller acts in, as JSON or
as a CSV export (`?format=csv`), filtered by actor, action and time. A
signed-in person needs `audit:read` (Owners and Admins); a key needs the
`admin` scope. An entry about an org's object (its budget, a key, an end
user, a member) is filed under that org, whoever made the change, so its
admins see the master key's changes to it too.

In hosted mode the master key acts in no org. Its changes to the gateway
(models, providers, routes, settings) and to accounts (`/v1/users`) concern
no org, and so does an account change by a person who belongs to none.
These entries are filed under no org and have no `org_id`. Only the master
key reads them, at the same endpoint; an org's members never see them. In
single-tenant mode they are filed under the default org, whose Owners and
Admins run the deployment, and the master key reads that org's log.

In either mode the master key reads any one org's log with `?org_id=`, as
the spend endpoints take it. Anyone else may name only the org they act in;
another is a 403.

An entry's `actor_label` is a signed-in person's email, an API key's id,
`master-key` for the master key or `system` for LiteGen itself (a budget
alert). Since 2026-11-15 an account's email must be an address, so no new
account reads as `master-key` or `system`.

Versions before 2026-11-15 filed the hosted master key's entries under the
default org. A hosted start moves them out of it
([RUNBOOK](RUNBOOK.md#operator-entries-in-the-default-orgs-audit-log)).

## Not carried over

- **LLM-only features:** guardrails, MCP, prompts, vector stores,
  fine-tuning, files, pass-through endpoints, tokens-per-minute limits.
- **Enterprise-only in LiteLLM, not built here:** IP allowlists,
  per-key route allowlists, SSO/SCIM, tag budgets, temporary budget increases,
  per-model budgets on a key.
- **Already covered by existing endpoints:** keys (create/rotate), users,
  invitations, orgs, members, audit log and cache.
