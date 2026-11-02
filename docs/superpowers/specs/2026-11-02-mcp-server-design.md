# LiteGen MCP server — design

Status: for approval, 2026-11-02. The user skipped the question round and
said "start", so every decision below marked **(call)** is mine, made from
the request and the code; each can be overridden.

## Intent

Make LiteGen a first-class MCP server, so an AI pipeline or an AI client
(Claude Code, Claude Desktop and claude.ai, ChatGPT, Codex, VS Code, Cursor
and the rest) can use LiteGen's whole API through MCP, nearly 1:1, for both
the AI (generation) endpoints and the management endpoints.

The user's three requirements, in their words:

1. "make a first-class supported mcp out of our functionality"
2. "it should share same api perms based on management api separated from
   ai api (like how litellm does it)"
3. "first-rate documentation on using and hooking up the mcp for every major
   mcp consumer like claude code, vs code, cursor, codex, chatgpt, etc"

## Goals

- **Hosted, zero install.** Every LiteGen deployment serves MCP itself over
  Streamable HTTP. Nothing to `npx`, nothing to keep in version sync.
- **Two endpoints, on LiteLLM's paths (user's choice: "copy litellms rest
  paths then to keep it familiar").** `{public_base_url}/mcp` serves the AI
  API (generate, quote, poll, models, generations, assets), like LiteLLM's
  `/mcp`. `{public_base_url}/admin/mcp`, the admin endpoint, serves the
  proxy's existing management API (keys, budgets, end users, spend, logs,
  audit, orgs/apps/members, gateway config), like LiteLLM's LiteAdmin MCP at
  `/admin/mcp`. It is not a place to manage MCP: MCP itself has only on/off
  switches. On the hosted service these are `https://app.litegen.ai/api/mcp`
  and `https://app.litegen.ai/api/admin/mcp`.
- **The same permissions as the HTTP API, by construction.** A tool call is
  executed as an internal HTTP request through the same axum router, with
  the same authentication, scope checks, tenant checks, budgets, rate limits,
  idempotency, audit log and (once they land) generation hooks. MCP adds no
  permission system of its own; it only hides tools the caller could not
  call.
- **Nearly 1:1.** 112 of the 132 OpenAPI operations become tools; the 20 left
  out are browser sign-in flows and probes (listed below). One tool is added:
  `wait_for_generation`, because an agent cannot sleep between polls.
- **Every major client connects natively.** Bearer API keys for clients that
  take headers; OAuth 2.1 (the MCP authorization spec) for ChatGPT,
  claude.ai / Claude Desktop connectors, and any client that prefers sign-in.
- **First-rate docs.** One page per client with tested setup steps, a
  permissions page, a generated tool reference, self-hosting and
  troubleshooting pages, and an in-product "Connect AI tools" page.

## Non-goals

- No change to the HTTP API's permission model beyond the three scope
  changes the user approved (see "Scope changes to the HTTP API").
- OAuth tokens are not a new way to call the REST API. They are valid on the
  MCP endpoints only.
- No stdio server, no npm/PyPI MCP package. Every client on the list speaks
  remote MCP; the docs cover a bridge only for a client that does not.
- No MCP prompts. No resource catalogue in v1 (tool results carry
  `resource_link`s to asset URLs instead).
- No MCP Tasks extension in v1. Only claude.ai and Claude Code support it
  today, and rmcp's task manager keeps state in one process's memory, which
  a stateless multi-replica server cannot share. `wait_for_generation`
  covers long jobs for every client; Tasks is the natural follow-up.
- No listing in Claude's connector directory: its review rejects
  connectors that generate images or video. LiteGen is added as a custom
  connector, which is unaffected.
- Not published to the MCP Registry by me: the `server.json` is prepared,
  publishing needs the user's domain verification.

## Anti-goals (failure even if it "works")

- A tool that lets a credential do anything its HTTP call would not.
- A tool list that shows tools the caller will always be refused.
- Cookie auth on `/mcp` (a web page could drive a logged-in browser's
  session through it).
- Docs whose config snippets were never run or checked against the client's
  current documentation.
- An OAuth flow that hands an MCP client a broader grant than the consenting
  user holds.

## Constraints

- Lives in `litegen-core` (Rust 2021, axum 0.8, tokio, utoipa 5); SQLite and
  Postgres both supported for every new table.
- Several Claude sessions share the main checkout. This work runs in its own
  git worktree on branch `mcp` (from `3d-models`), with its own `target/`.
  Peers are told before any OpenAPI-visible change lands.
- `AppState` is constructed literally in many test files; this work adds no
  field to it. MCP settings live in `AppConfig.mcp` (reached through
  `state.router.config`).
- The mesh library rule (`.claude/rules/mesh-standalone.md`) is untouched:
  MCP code never reaches into `src/mesh/`.
- Commits only with the user's go; never Claude attribution anywhere.

## Architecture

```
MCP client ──HTTP POST /mcp (JSON-RPC)──▶ mcp::auth  (Bearer → Caller; 401 + WWW-Authenticate)
                                          mcp::server (rmcp ServerHandler, stateless)
                                             tools/list → mcp::registry (OpenAPI + mcp::catalog, filtered by mcp::gate)
                                             tools/call → mcp::dispatch ──oneshot──▶ the API Router
                                                                          (auth_middleware, check_scope,
                                                                           CSRF, idempotency, handlers…)
                                                          ◀── HTTP response ── mcp::content (→ CallToolResult)
```

- **Protocol layer (call):** the official Rust SDK, `rmcp` 3.8
  (`server` + `transport-streamable-http-server` features), as a
  `StreamableHttpService` mounted with `Router::nest_service`, with
  `NeverSessionManager`, `legacy_session_mode(false)` and
  `json_response(true)`. That serves MCP 2026-07-28 (stateless, no
  `initialize`, `server/discover`) and every older revision a client
  negotiates, each POST on its own, so any replica can answer any request;
  GET and DELETE answer 405. Each request's HTTP parts, carrying the
  resolved caller, reach the handler through `RequestContext.extensions`
  (`ctx.extensions.get::<http::request::Parts>()`). Research behind this:
  [spec, rmcp and LiteLLM notes](../research/2026-11-02-mcp-spec-rmcp-litellm.md).
- **Host and Origin checks:** rmcp rejects a `Host` it does not know (its
  default is loopback only, which would 403 every hosted request), so the
  allowed hosts are loopback, the host of `public_base_url`, and
  `mcp.allowed_hosts` (`"*"` turns the check off). Origin validation is
  enforced (the spec's MUST): a request with an `Origin` must come from the
  CORS origins, the origin of `public_base_url`, or `mcp.allowed_origins`;
  requests without one (every non-browser client) pass. Request bodies up to
  16 MiB (base64 mesh uploads). SSE responses carry `X-Accel-Buffering: no`.
- **Registry:** built once at startup from `ApiDoc::openapi()` (the same
  utoipa document `/openapi.json` serves) plus a hand-curated table
  (`mcp::catalog`) that names each tool, assigns its endpoint and toolset,
  and states its gate. A contract test fails when an OpenAPI operation is
  neither a tool nor explicitly excluded, so a new endpoint can never be
  silently missing from MCP.
- **Input schema:** path, query and `Idempotency-Key` parameters plus the
  JSON body's properties, flattened into one object at the root (no root
  `allOf`/`oneOf`: Claude Code flattens those and loses `required`);
  `$ref`s inlined; `["T","null"]` collapsed to `T`; binary formats dropped.
  Every schema must validate against the JSON Schema 2020-12 meta-schema
  and every property name must match `[A-Za-z0-9_.-]{1,64}` — Claude Code
  silently drops a tool that fails either, so a test checks all of them.
  Generation tools also take `end_user` and `tags` (sent as
  `x-litegen-end-user` / `x-litegen-tags`).
- **Annotations:** every tool declares `title`, `readOnlyHint`,
  `destructiveHint`, `idempotentHint` and `openWorldHint` (claude.ai's
  connector rules require the first two; ChatGPT asks for confirmation on
  any tool not marked read-only). GETs, the three cost quotes and
  `wait_for_generation` are read-only; deletes, revokes, blocks, rotations,
  resets and ownership transfers are destructive; generation tools are
  open-world (they call outside vendors and spend money).
- **Dispatch:** builds an `http::Request` (path parameters percent-encoded,
  except catch-all ids such as model ids, which keep their `/`), copies the
  caller's `Authorization` header for API keys, or attaches the resolved
  principal as a request extension for OAuth tokens, forwards
  `X-Litegen-Org-Id` / `X-Litegen-App-Id`, and adds the spend tag `mcp`.
  Then `api_router.clone().oneshot(req)`.
- **Results:** a JSON success becomes `structuredContent` plus the same JSON
  as text; a non-2xx becomes `isError: true` with the API's status, code and
  message (so the model can correct itself); `x-litegen-dropped-params` is
  surfaced as a note. Generated images come back as small previews (MCP
  image content, longest side ≤ 512 px, JPEG, at most 2 per call, turned off
  with `preview: false`) — Claude Code counts images against its
  25,000-token output cap — plus a `resource_link` per full-size URL;
  meshes and videos come back as `resource_link`s; a binary response with
  no URL (a converted mesh) as an embedded resource up to 8 MiB.
- **`wait_for_generation`:** polls the job's own status route (`/v1/videos/{id}`
  for `litegen-vid-…`, `/v1/models3d/{id}` for `litegen-3d-…`, otherwise
  `/v1/generations/{id}`) through the dispatcher, emits
  `notifications/progress` when the client sent a progress token, and
  returns the final state or, at its timeout, the latest state with a note
  to call again. The default timeout is 45 s, under Claude Code's 60 s
  per-request limit; the maximum is 230 s, under claude.ai's 240 s.
- **Server instructions:** each endpoint returns a short usage guide in
  `initialize` (find a model → read its schema → quote → generate → wait).
  The first 512 characters stand alone (ChatGPT and Codex keep only those);
  the whole text stays under 4,096 (Claude Code's cut).

## Permissions

### Principals (all existing)

| Credential | Acts as | Scopes | Where it works |
|---|---|---|---|
| API key (`Authorization: Bearer sk_live_…`) | the key | the key's `generate` / `read` / `admin` | both endpoints |
| Master key | platform admin | all | both endpoints |
| Dev mode (single-tenant, no master key) | open, as the HTTP API is | all | both endpoints |
| OAuth access token (new) | the user who approved it, in the org/app they picked, with their role there | the scopes they approved (`ai:*` or `admin:*`, mapped to `generate`/`read`/`admin`), never more than they could grant a key | the one endpoint it was issued for |
| Session cookie | — | — | ignored on `/mcp` |

### Gates

Each tool has a gate that mirrors the check its route performs, so
`tools/list` shows exactly what the credential can call:

| Gate | Mirrors | Visible when |
|---|---|---|
| `scope:generate` / `scope:read` / `scope:admin` | `check_scope` on the route group | the credential holds that scope |
| `any-scope(generate, read)` | `check_any_scope` on the AI read routes (new) | the credential holds either |
| `org-read(perm)` / `org-write(perm)` | `management::require_org` | platform admin; a user whose role has `perm` (and, for OAuth, the `read` / `admin` scope); a key with `read` or `admin` / `admin` |
| `key-read` / `key-write` | the key-limits handlers | as `org-read(KeyReadOwn)` / `org-write(KeyWriteOwn)` |
| `member(perm)` | `orgs::require_member_perm` (session-only today) | platform admin, or a user whose role has `perm` (+ OAuth scope) |
| `global(perm)` | `users::authorize_global_user_admin` | hosted: platform admin; single-tenant: a user with `perm` |
| `gateway-admin` | `management::require_gateway_admin` | as that function decides |
| `user` | handlers that need a signed-in user | a user principal (OAuth) |
| `any` | any authenticated caller | always |

Calling a tool that is hidden returns "unknown tool". The handler's own
checks still run on every call (for example, the path's org must be the
caller's).

### LiteLLM comparison

LiteLLM separates its LLM API from its management API with virtual-key
types (`llm_api`, `management`, `read_only`, `default`), each a preset of
allowed route groups (`llm_api_routes`, `management_routes`,
`info_routes`). Its MCP follows the same line: MCP tool calls are data-plane
(`llm_api_routes`), and its management API is exposed as MCP on a separate
endpoint, LiteAdmin MCP at `/admin/mcp`, whose tools come from LiteLLM's own
`/openapi.json` filtered through a reviewed catalog, are called in-process,
and are shown per caller by the same route checker the HTTP API uses. That
is this design, on the same two paths: `/mcp` for the data plane and
`/admin/mcp` for the management API, OpenAPI + curated catalog, in-process
dispatch, the HTTP checks deciding visibility. Two differences: LiteAdmin requires a `proxy_admin` key for every
tool, while LiteGen gates each tool by the scope or role its route needs;
and LiteAdmin has no OAuth, while LiteGen needs it for ChatGPT and claude.ai.
LiteGen's key-level equivalents, after the scope changes below:

| LiteLLM key type | LiteGen key (Keys page preset) | LiteGen MCP |
|---|---|---|
| `llm_api` | `generate` ("AI") | the `/mcp` tools; on `/admin/mcp` only `get_my_limits` |
| `management` | `read,admin` ("Management") | the `/admin/mcp` tools, plus `/mcp` read tools |
| `read_only` | `read` ("Read-only") | read tools on both endpoints |
| `default` | `generate,read,admin` ("AI + management") | everything |

### Scope changes to the HTTP API (user-approved, 2026-11-02)

Asked whether a key can be granted "both, just management or just AI", the
answer was "not cleanly", for two reasons, plus a bug. All three are fixed
first (plan 1, Task 0), and MCP follows from them:

1. **AI-only keys.** Today `generate` alone cannot list models or read its
   own generations (those routes need `read`), and `read` also opens the
   org's logs, stats, spend, limits and end users. Change: the AI read routes
   (`/v1/models`, `/v1/models/{id}`, `/v1/providers`,
   `/v1/utils/models3d/formats`, `/v1/generations…`) accept `generate` **or**
   `read`. A `generate` key is then exactly "AI". Every existing key keeps
   everything it can do today; keys created without `scopes` still default
   to `generate,read`.
2. **The Keys page offers LiteLLM-style presets** (AI, Management,
   AI + management, Read-only, Custom) instead of only a free-text field,
   in both the create and edit forms. The presets that include `admin`
   appear only to people who can grant it. The text field stays, kept in
   sync with the preset.
3. **Bug: editing a key skipped the privilege check.**
   `PATCH /v1/keys/{id}` wrote `scopes` without the check `POST /v1/keys`
   applies, so a Member could create a `generate,read` key and then edit it
   to `admin`. An admin-scope key can manage every key in the org, change
   budgets and end users, and in single-tenant mode change gateway
   config. Edits also accepted misspelled scopes silently. Edits now get the
   same check and unknown-scope rejection as creation.

## OAuth 2.1 for MCP (plan 2)

LiteGen becomes its own authorization server for its two MCP resources,
following the MCP authorization spec (revision 2026-07-28). ChatGPT cannot
send an API key or any custom header, and most claude.ai / Claude Desktop
users cannot either (request headers are a limited beta), so for those
clients OAuth is the only way in. Client facts behind each choice:
[research notes](../research/2026-11-02-mcp-clients.md).

- **Scopes name their endpoint (call).** `ai:generate`, `ai:read` (for
  `/mcp`) and `admin:read`, `admin:write` (for
  `/admin/mcp`). Each maps 1:1 to a key scope on one endpoint. Prefixed
  names let the server tell which endpoint a grant is for even when a client
  sends the bare origin as `resource` (ChatGPT's docs show exactly that).
- **Discovery.** `401` from `/mcp` carries
  `WWW-Authenticate: Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp", scope="ai:generate ai:read"`
  (a real 401: Claude ignores the header on any other status).
  Protected-resource metadata (RFC 9728) per endpoint, `resource` = the
  endpoint URL exactly. Authorization-server metadata (RFC 8414), issuer =
  `public_base_url`, advertising `code_challenge_methods_supported:["S256"]`,
  `token_endpoint_auth_methods_supported:["none"]`,
  `client_id_metadata_document_supported:true`, a `registration_endpoint`,
  and `authorization_response_iss_parameter_supported:true`. Both documents
  are served at every path a client may derive (`/.well-known/…/mcp`,
  `/.well-known/…/api/mcp`, and the bare well-known paths), and
  `deploy/Caddyfile` routes the root-level `/.well-known/oauth-*` paths to
  litegen.
- **Clients.** Client ID Metadata Documents first (an `https` `client_id`
  whose JSON is fetched with a size cap, a timeout and an SSRF guard, then
  cached) — Claude Code, claude.ai and ChatGPT all pick it when advertised —
  and Dynamic Client Registration (RFC 7591, deprecated in the spec but
  still used) for the rest. Public clients only; PKCE S256 required;
  redirect URIs must be ones the client declared, matched exactly, except
  loopback (`localhost`, `127.0.0.1`, `[::1]`) which matches any port
  (RFC 8252; Claude Code and Codex use it).
- **Consent.** `/oauth/authorize` validates the request, stores it, and
  redirects to the dashboard page `/oauth/consent`. The signed-in user picks
  the org and app and the scopes (pre-filled from the request, limited to
  what they could grant a key), then approves or denies. Every redirect back
  to the client, errors included, carries `iss` (RFC 9207): Claude Code's
  runtime rejects a mismatch and ChatGPT's stable redirect URI needs it.
- **Which endpoint a grant is for.** `resource` equal to an endpoint URL
  decides it. Otherwise the requested scopes decide (`ai:*` → `/mcp`,
  `admin:*` → `/admin/mcp`). Otherwise `/mcp`. A request mixing
  both prefixes is refused (`invalid_scope`): one grant, one endpoint.
- **Tokens.** Opaque, stored as SHA-256 hashes. Access tokens last 1 hour;
  refresh tokens 30 days and rotate on use (reuse of a rotated token revokes
  the grant; a dead one answers `invalid_grant`). Every token works on its
  grant's endpoint only, and never on the REST API. The token endpoint takes
  `application/x-www-form-urlencoded`. `POST /oauth/revoke` (RFC 7009).
- **Grants are visible and revocable.** `GET /v1/oauth/grants` and
  `DELETE /v1/oauth/grants/{id}` (in OpenAPI, both SDKs, the dashboard's
  "Connected apps", and tools on `/admin/mcp`).

## Configuration (`mcp:` in litegen.yaml)

```yaml
mcp:
  enabled: true                 # serve /mcp (the AI endpoint)
  admin_enabled: true           # serve /admin/mcp (like LiteLLM's LITELLM_ENABLE_ADMIN_MCP)
  allowed_hosts: []             # extra Host values; loopback and public_base_url's host are always allowed; ["*"] = no check
  allowed_origins: []           # extra browser Origins; CORS origins and public_base_url's origin are always allowed
  oauth:                        # plan 2
    enabled: true               # authorization server + discovery documents
    consent_url: null           # default: origin of public_base_url + /oauth/consent
    access_token_ttl_secs: 3600
    refresh_token_ttl_days: 30
```

## Docs and in-product surfaces

- `docs/mcp/README.md` — what it is, both URLs, quick start for the top
  clients, toolsets, long-running jobs, billing.
- `docs/mcp/permissions.md` — the tables above, worked examples.
- `docs/mcp/tools.md` — generated from the registry
  (`cargo run --example mcp_tools_doc`); a test fails when it is stale.
- `docs/mcp/clients/` — one page each: Claude Code; Claude Desktop and
  claude.ai; ChatGPT; Codex (CLI and IDE); VS Code (GitHub Copilot) and
  Visual Studio; Cursor; Windsurf; Gemini CLI; Zed; JetBrains (AI Assistant,
  Junie); other clients (Cline, Continue, Goose, Copilot coding agent,
  Kiro); APIs and SDKs (OpenAI Responses API and Agents SDK, Anthropic
  Messages API MCP connector and Claude Agent SDK, the official MCP SDKs);
  MCP Inspector. Each page: API-key setup, OAuth setup where the client has
  it, a verification prompt, the client's tool-count and timeout limits and
  which toolsets to pick, and the date and sources it was checked against.
- `docs/mcp/self-hosting.md`, `docs/mcp/troubleshooting.md`.
- Dashboard: `/connect` "Connect AI tools" page (both URLs, per-client
  snippets with copy buttons, "Add to Cursor" / "Add to VS Code" install
  links, a one-click MCP key, connected apps), and `/oauth/consent`.
- Root `README.md`, `sdks/README.md` and `docs/management-api.md` link to
  `docs/mcp/`.
- `server.json` for the MCP Registry (remote `streamable-http` entries for
  both endpoints), not published.

## Toolsets

`?toolsets=a,b` on either URL narrows the list, for clients that cap tool
counts. AI: `images`, `videos`, `models3d`, `catalog`, `generations`,
`account`. Admin endpoint: `keys`, `budgets`, `end_users`, `spend`, `logs`,
`audit`, `orgs`, `members`, `apps`, `users`, `gateway`, `system`, `account`,
`oauth`. Unknown names are a 400 with the valid list.

## Excluded operations (20)

Browser sign-in and session flows: `/auth/redirect`, `/v1/auth/config`,
`/v1/auth/csrf`, `/v1/auth/login`, `/v1/auth/logout`, `/v1/auth/signup`, both
password-reset routes, the four GitHub/Google OAuth start/callback routes.
Invitation links (`GET /v1/auth/invitations/{token}`, `…/accept`): meant for a
person clicking an emailed link. Account self-service (`/v1/account`,
`/v1/account/sessions`, `…/{id}`): browser-session settings. Probes
(`/health/live`, `/health/ready`).

## Verification

- Rust unit tests (schema, catalog contract, gates, dispatch, results, wait)
  and integration tests over the real router with mock providers.
- OAuth integration tests on SQLite, and the store against Postgres in
  Docker.
- Real clients: Claude Code (`claude mcp add` / `claude mcp list` against a
  local server), MCP Inspector CLI (`tools/list`, `tools/call`), the official
  MCP TypeScript SDK client including a full OAuth run, and the Gemini and
  Codex CLIs via `npx` where they can check a connection without an account.
  Clients that cannot be run here (Cursor, Windsurf, Zed, JetBrains,
  ChatGPT, claude.ai) are checked against their current official docs, and
  their pages say so with the date.

## Left to the builder

Internal module layout beyond the files the plans name, error wording, the
exact preview size cap (≤ 1 MiB per image, ≤ 4 images per call), log/tracing
spans, and test fixture shapes.
