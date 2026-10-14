# Management API

LiteGen's management API covers what the LiteLLM proxy's management endpoints
do, mapped onto LiteGen's own model: orgs ≈ LiteLLM organizations, apps ≈ teams,
API keys ≈ virtual keys, and end users ≈ customers. Every endpoint is in
`/openapi.json`, in both SDKs, and used by the dashboard.

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
(a lifetime USD cap) still applies as well.

The org's allowed-model pool and the app's model access
(`/v1/orgs/{id}/allowed-models`, `/v1/apps/{id}/allowed-models`) are now
enforced at generation time. `GET /v1/models` lists only the models the caller
may use.

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

## Gateway admin

These endpoints are for the platform admin only, plus owners/admins and
`admin`-scope keys in single-tenant mode. Changes apply to the next request and
are stored, then re-applied at boot.

| Endpoint | LiteLLM | |
|---|---|---|
| `GET/POST /v1/admin/models` | `/model/info`, `/model/new` | Add a model at runtime. The body is a `models/*.yaml` row and goes through the same validation. The vendor model called is the part of the id after `<provider>/`. |
| `GET/PUT/PATCH/DELETE /v1/admin/models/{id}` | `/model/update`, `/model/block`, `/model/delete` | `PATCH {disabled}` hides any model, catalog or runtime. Catalog models are otherwise read-only. |
| `GET /v1/admin/providers`, `GET/PUT/DELETE /v1/admin/providers/{name}` | `/credentials` | Set keys, `api_base`, `model_mapping`, headers, options or `enabled`. Secrets are AES-GCM encrypted at rest and only shown as `…last4`. Unset fields fall back to the env/YAML config. In-flight jobs keep the instance they started with. |
| `GET/PUT/DELETE /v1/admin/routes` | router settings, fallbacks | Model routes: fallback, weighted round robin, lowest cost, lowest latency, caching |
| `GET/PUT /v1/admin/settings` | `/config/update` | `cost_markup_percent`, `alert_webhook_url`, and a default end-user budget |

Runtime models are refused for `fal`, whose adapter maps model ids through a
fixed endpoint table. Add fal models to `models/fal.yaml` instead.

## Not carried over

- **LLM-only features:** guardrails, MCP, prompts, vector stores,
  fine-tuning, files, pass-through endpoints, tokens-per-minute limits.
- **Enterprise-only in LiteLLM, not built here:** IP allowlists,
  per-key route allowlists, SSO/SCIM, tag budgets, temporary budget increases,
  per-model budgets on a key.
- **Already covered by existing endpoints:** keys (create/rotate), users,
  invitations, orgs, members, audit log and cache.
