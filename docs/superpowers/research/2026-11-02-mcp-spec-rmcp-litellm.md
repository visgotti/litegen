<!-- Research notes for docs/superpowers/specs/2026-11-02-mcp-server-design.md, gathered 2026-11-02 by a research agent from LiteLLM source, the MCP spec and the rmcp 3.8.0 source. Model-written; verify before relying on any single claim. -->

# LiteGen MCP research report (as of 2026-11-02)

**Reading guide.** No files were created, edited or deleted.
- Unmarked facts were read in primary text: raw source files, spec markdown, crates.io JSON.
- `(p)` means the fact came from a docs-site page read through a fetch/summarize tool. Wording may be paraphrased, so treat it as less authoritative than source.
- "unverified" means nothing I read says so.
- `[tag]` resolves to a URL in the source table at the end of each section.
- rmcp line numbers are for git tag `rmcp-v3.8.0`.
- Items marked "(inference)" are my conclusions, not source statements.

---

## 0. Headlines

1. **The current MCP revision is 2026-07-28, not 2025-11-25.** [S-ver] says "The current protocol version is 2026-07-28".
   - It removes `initialize` and `Mcp-Session-Id`, so the protocol is stateless. [S-chg]
   - Tasks became an extension (`io.modelcontextprotocol/tasks`, SEP-2663). [S-chg]
   - `Mcp-Method` and `Mcp-Name` headers are now required. [S-chg]
   - Server-initiated requests were replaced by MRTR. [S-chg]
   - DCR is deprecated in favour of CIMD. [S-chg]
   - 2025-11-25 is the previous revision.
   - Claude's connector docs still cite auth specs up to 2025-11-25 `(p)` [C-bld]. Claude Code's v2 runtime adds 2026-07-28 `(p)` [CC]. rmcp 3.8.0 serves both families [R-model:167-207].
2. **LiteLLM separates planes by route-group allowlists, not by separate servers.**
   - `key_type` has exactly four values: `llm_api`, `management`, `read_only`, `default`.
   - Three of them only write a preset `allowed_routes` (`llm_api_routes`, `management_routes`, `info_routes`). [A-kme]
   - MCP tool-call routes are in `llm_api_routes`. MCP-server CRUD is in `management_routes`. [A-typ]
   - LiteLLM does expose its management API as MCP tools: Enterprise "LiteAdmin MCP" at `/admin/mcp`. It is a separate endpoint, `proxy_admin`-only, and reuses the HTTP route checker. [A-adm][D-la]
3. **rmcp 3.8.0 (MSRV 1.88).** `StreamableHttpService` mounts with `Router::nest_service`.
   - A handler reads auth context through `ctx.extensions.get::<http::request::Parts>()`.
   - There is no server-side protected-resource-metadata (PRM) helper, so you hand-roll it.
   - `allowed_hosts` defaults to loopback only, so a public Host header gets 403 until you override it.
   - `allowed_origins` defaults to empty, which means no Origin check.
4. **Long generation jobs vs client timeouts.**
   - Codex `tool_timeout_sec` defaults to 60 `(p)` [CX].
   - claude.ai/Desktop allow about 240 s per call `(p)` [C-rev].
   - The community client matrix shows Tasks only for Claude web, Claude Code and MCPJam [S-mat].
   - A submit-then-poll tool pair is the universal fallback (inference).
5. **Anthropic's connector directory review rejects connectors that "Generate images, video, or audio through AI models"** `(p)` [C-rev]. That concerns directory listing. Custom connectors are presumably unaffected (inference).

---

## A. LiteLLM: management API vs LLM API

### A1. `key_type` on `/key/generate`, and `allowed_routes`

The enum is exhaustive. There are four values and no others. [A-key]
```python
class LiteLLMKeyType(str, enum.Enum):
    LLM_API = "llm_api"        # Can call LLM API routes (chat/completions, embeddings, etc.)
    MANAGEMENT = "management"  # Can call management routes (user/team/key management)
    READ_ONLY = "read_only"    # Can only call info/read routes
    DEFAULT = "default"        # Uses default allowed routes
```
`GenerateKeyRequest.key_type: LiteLLMKeyType | None = Field(default=LiteLLMKeyType.DEFAULT, description="Type of key that determines default allowed routes.")`. `KeyRequestBase.allowed_routes: list[Any] | None = []`. There are also `allowed_passthrough_routes` and `denied_passthrough_routes`. [A-key]

The preset mapping lives in `key_management_endpoints.py`. [A-kme]
```python
_KEY_TYPE_PRESET_ROUTES = MappingProxyType({
  LiteLLMKeyType.LLM_API:    ("llm_api_routes",),
  LiteLLMKeyType.MANAGEMENT: ("management_routes",),
  LiteLLMKeyType.READ_ONLY:  ("info_routes",),
})
# handle_key_type(): data_json["allowed_routes"] = list(preset_routes)   # DEFAULT => no preset
```

| key_type | Stored `allowed_routes` | What it can reach (membership in A2) |
|---|---|---|
| `llm_api` | `["llm_api_routes"]` | OpenAI, Anthropic, Google, pass-through, MCP data-plane, native and model-info routes |
| `management` | `["management_routes"]` | User, team, model and key CRUD, MCP-server CRUD, agent CRUD |
| `read_only` | `["info_routes"]` | `/key/info`, `/user/info`, `/team/info`, `/model/info`, `/health`, list routes |
| `default` | none | No key-level restriction. The Admin UI labels it "Full Access". It skips `default_key_generate_params.allowed_routes`. |

Interaction with `allowed_routes`:
- **Docstring.** `allowed_routes` takes routes or wildcard patterns, e.g. `["/chat/completions","/embeddings","/keys/*"]`. When `key_type` is omitted, the key gets `default_key_generate_params.allowed_routes` if configured and the request has no `allowed_routes`; otherwise it behaves like `default`. [A-kme]
- **Docs on presets.** "The `management` and `read_only` key types always keep their presets, and a `key_type` of `default` ... keeps full access". `default_key_generate_params.allowed_routes` (e.g. `["llm_api_routes","/key/spend/report"]`) "is an allowlist". [D-vk] `(p)`
- **Who may set raw routes.** `_check_allowed_routes_caller_permission` lets only `PROXY_ADMIN` send raw `allowed_routes`. Non-admins can only pick presets whose group is in `_NON_ADMIN_SAFE_ALLOWED_ROUTES_PRESETS = {"llm_api_routes","info_routes"}`, via `key_type`. The error text is "Only proxy admins can set `allowed_routes` on a key. Use `key_type` to pick a preset route bucket instead." [A-kme]
- **Enforcement.** `RouteChecks.is_virtual_key_allowed_to_call_route` applies to all roles including `proxy_admin`. [A-rc] The docs say it "is enforced for every role, including admin-owned keys". [D-vk] `(p)`
  - Empty or None `allowed_routes` means allowed.
  - Matching goes in order: exact or `route.startswith(allowed + "/")`, then route-group names (`LiteLLMRoutes._member_names_`) via `check_route_access` (exact, trailing-`*` wildcard, `{param}` pattern), then wildcard patterns.
  - Carve-out: `llm_api_routes` keys may GET `/v1/mcp/server` and `/v1/mcp/server/{id}` for discovery, with the response sanitized.
  - Otherwise 403 "Virtual key is not allowed to call this route. Only allowed to call routes: ...".
- **Owner role still applies.** Key routes only narrow. "If the owner is `proxy_admin`, every non-admin route restriction is skipped and the key can manage keys, users, and teams". [D-vk] `(p)`
- **Ambiguity (unverified).** The pydantic default is `DEFAULT`, but the docstring distinguishes "omitted" from explicit `default`. I did not trace how the code tells them apart.

### A2. Route groups (`class LiteLLMRoutes(enum.Enum)`, `_types.py` ~L229-1050) [A-typ]

| Group | Contents (abridged) |
|---|---|
| `openai_routes` | `/chat/completions`, `/completions`, `/embeddings`, `/images/generations`, `/images/edits`, `/videos*`, audio, moderations, batches, files, fine_tuning, assistants/threads, models, `/utils/token_counter`, rerank, realtime, responses, vector stores, search, OCR, containers |
| `anthropic_routes`, `google_routes` | `/v1/messages` etc.; Google routes |
| `mapped_pass_through_routes` (+ `passthrough_routes_wildcard` = `f"{route}/*"`) | Provider pass-through routes |
| `mcp_inference_routes` | **Data plane**, "Gated by DISABLE_LLM_API_ENDPOINTS": `/mcp`, `/mcp/`, `/mcp/sse/`, `/mcp/sse/messages`, `/mcp/proxy`, `/mcp/{subpath}`, `/mcp/tools`, `/mcp/tools/list`, `/mcp/tools/call`, `/mcp-rest/tools/list`, `/mcp-rest/tools/call`, `/v1/mcp/tools`, `/introspect`, `/token` |
| `mcp_management_routes` | **Control plane**, "Gated by DISABLE_ADMIN_ENDPOINTS": `/v1/mcp/server`, `/v1/mcp/server/{path:path}`, `/v1/mcp/sessions` |
| `mcp_routes` | `mcp_inference_routes + mcp_management_routes` (back-compat union) |
| `agent_inference_routes`, `agent_management_routes`, `agent_routes` | Same split for agents |
| `llm_api_routes` | `openai + anthropic + google + mapped_pass_through + passthrough_wildcard + apply_guardrail + mcp_inference + litellm_native + agent_inference + model_info`. Also `llm_api_routes_without_mcp = sorted(set(llm_api_routes) - set(mcp_inference_routes))`. |
| `info_routes` | `/key/info`, `/key/health`, `/team/info`, `/team/list`, `/v2/team/list`, `/organization/list`, `/team/available`, `/team/metadata_schema`, `/user/info`, `/v2/user/info`, `/management/v1/users/me`, `/model/info` (+v1/v2), `/v2/key/info`, `/model_group/info`, `/health`, `/health/services`, `/key/list`, `/user/filter/ui`, `MODEL_LIST_ROUTES`, `/sso/get/ui_settings`, `/get/user_banner`, `/get/latest_release_info` |
| `management_routes` | `/user/{new,update,delete,info,list}`, `/team/{new,update,delete,list,info,block,unblock,permissions_*}`, `/model/{new,update,delete,info}`, `/jwt/key/mapping/*`, plus `key_management_routes` (the `KeyManagementRoutes` enum in `user_roles.py`), `mcp_management_routes` and `agent_management_routes` |
| Others | `master_key_only_routes` (`/global/spend/reset`, memory-usage); `spend_tracking_routes`; `global_spend_tracking_routes`; `public_routes` (`/routes`, `/`, `/health/liveliness`, `/health/liveness`, `/test`, `/.well-known/litellm-ui-config`, `/public/model_hub*`, `/public/mcp_hub`); `internal_user_routes`; `internal_user_view_only_routes`; `self_managed_routes`; `org_admin_only_routes`; `admin_viewer_routes`; `org_admin_allowed_routes` |

Docs table for JWT route groups `(p)` [D-tok]:
- Defaults: `admin_allowed_routes` = management + spend + global_spend + info; `team_allowed_routes` = openai + info + mcp + `/v1/messages` (+`/count_tokens`); `public_allowed_routes` = public.
- Only a trailing `*` is a wildcard.
- Docs describe `management_routes` as "Admin-only management endpoints (create/update/delete user/team/model)" and `info_routes` as "Read-only & info endpoints used by the UI".

### A3. Roles and how they gate management routes

| Role (`LitellmUserRoles`) [A-rol] | Docs description `(p)` [D-ac] |
|---|---|
| `proxy_admin` | "admin over litellm proxy, has all permissions" |
| `proxy_admin_viewer` | "view all keys, view all spend"; cannot create or delete keys or add users |
| `org_admin` | Enterprise; org-level admin |
| `internal_user` | View, create and delete own keys; view own spend |
| `internal_user_viewer` | View own keys and spend; docs mark it DEPRECATED |
| `team` | JWT auth |
| `customer` | End-user role |

Team member `role` is `admin` or `user`. Team admin is a Premium feature; default team-member permissions are `/key/info` and `/key/health`, and extra ones such as `/key/generate` and `/key/delete` can be granted. [D-ac] `(p)`

Gating order. `non_proxy_admin_allowed_routes_check` runs only when the user is not a proxy admin (called from `auth_checks._is_api_route_allowed`). The order is: [A-rc][A-ac]
1. Custom `admin_only_routes` (enterprise).
2. Auth-enforced pass-through.
3. LLM-API routes pass.
4. Info routes pass (with a `/user/info` user_id check).
5. `global_spend_tracking_routes`, requiring the `get_spend_routes` permission.
6. `PROXY_ADMIN_VIEW_ONLY` via `_check_proxy_admin_viewer_access`. It returns 403 on LLM-API routes and blocks writes through `_PROXY_ADMIN_VIEW_ONLY_BLOCKED_ROUTES`. GET/HEAD/OPTIONS are default-allowed on non-inference routes.
7. `INTERNAL_USER` with `internal_user_routes` and self-scoped credentials; org admin with `org_admin_allowed_routes`; `INTERNAL_USER_VIEW_ONLY` with `internal_user_view_only_routes`; any role with `self_managed_routes`.
8. `/v1/mcp/*` and `/mcp-rest/*` ("authN/authZ handled by api itself").
9. Pass-through and team service-account key routes, then the key's `allowed_routes`.
10. Otherwise 401 `_raise_admin_only_route_exception` ("Authentication Error, Only proxy admin can be used to generate, delete, update info for new keys/users/teams...").

### A4. LiteLLM's MCP gateway: how key/team permissions apply

**Auth to the gateway.** [A-hdr][A-mcpauth][D-auth] `(p)`
- Primary header is `x-litellm-api-key: Bearer sk-...`. `Authorization: Bearer` is the secondary.
- Vendor aliases (`x-api-key`, `API-Key`, etc.) work on MCP REST and A2A routes but not on MCP ASGI routes.
- `SpecialHeaders` also defines `mcp_auth="x-mcp-auth"` (deprecated), `mcp_servers="x-mcp-servers"` and `mcp_access_groups="x-mcp-access-groups"`.
- Per-server upstream credentials go in `x-mcp-{alias}-authorization` (and `x-mcp-{access_group}-{header}` per docs). [D-mcp] `(p)`
- Source `AGENTS.md`: "Preserve the boundary between LiteLLM admission auth and upstream MCP auth." [A-agents]

**Server selection.** `x-mcp-servers` takes aliases, server IDs or access groups. "If the header is not provided, tools from all available MCP servers will be accessible." Path namespacing `/{server}/mcp` also exists. [D-ctl][D-mcp] `(p)`

**Permission model.** [D-ctl] `(p)`
- Open by default.
- `object_permission.mcp_servers`, `mcp_access_groups` and `mcp_tool_permissions` (server ID to tool names) are intersected across key and team.
- Org is a ceiling. End user, agent and internal user narrow further. [D-auth] says six levels are resolved.
- Enforced at both `tools/list` and `tools/call`.
- Per-server `allowed_tools` / `disallowed_tools` (allowed wins) and `allowed_params`.
- `require_key_mcp_access_defined: true` makes keys closed by default. Sentinel `no-mcp-servers`.
- `allow_all_keys: true` makes a server public to all keys. [D-mcp] `(p)`
- `mcp_rpm_limit` exists.
- The source contract fails closed when entitlements cannot be loaded. [A-mcpauth]

**MCP OAuth.** The gateway serves `.well-known/oauth-protected-resource` and `.well-known/oauth-authorization-server`, and proxies or forwards DCR (RFC 7591) to the upstream AS. A cold-start 401 carries `WWW-Authenticate: Bearer resource_metadata="..."`. [D-mcp] `(p)` [A-mcpauth]

**Other gateway behaviour.**
- Pagination of tools, prompts and resources uses encrypted cursors; it needs `LITELLM_SALT_KEY`, and cursors expire after 10 minutes.
- Elicitation is off by default (`allow_elicitation`).
- Verified over `/mcp/sse`; Streamable HTTP `/mcp` is not stated as verified. [A-mcpreadme]

**Plane classification (verified in code).** Tool-call routes are data-plane (`llm_api_routes`). Server and session CRUD are control-plane (`management_routes`). A `management` key therefore cannot reach `/mcp` tool calls (inference from group membership; not tested).

### A5. Does LiteLLM expose its management API as MCP tools? Yes: LiteAdmin MCP

- Standalone connector, repo `BerriAI/litellm-admin-mcp` (Python >=3.12, MIT). It runs over stdio (`uvx`) or Streamable HTTP at `/mcp` with `/healthz`. [G-la] `(p)`
- Embedded at **`/admin/mcp`** in LiteLLM Enterprise >= v1.106.0. Enable it with `LITELLM_ENABLE_ADMIN_MCP=true` plus `LITELLM_LICENSE` and `PROXY_BASE_URL`. An existing MCP alias named `admin` must be renamed. [D-lae] `(p)` [A-adm]
- The source mounts `Mount("/admin", ...)` and requires an enterprise license. Config comes from env: `LITELLM_ADMIN_READ_ONLY`, `LITELLM_ADMIN_TOOLS`, `LITELLM_ADMIN_RESPONSE_VIEW`, `LITELLM_ADMIN_SCHEMA_MODE`. [A-adm]
- Tools are discovered from the gateway's `/openapi.json` and filtered against a reviewed catalog. [D-la][D-blog] `(p)`
  - The catalog has 65 operations plus 2 helpers (`describe_admin_tool`, `read_admin_result`).
  - Names are like `create_key`, `add_model`, `list_teams`, `update_budget`.
  - Arguments are grouped as `body`, `query` and `path`.
- Auth is a bearer API key only (no browser OAuth) of a **`proxy_admin`** user, "for all tools, including reads". Viewer and team-admin users cannot connect. [D-la] `(p)`
- Read-only mode (`LITELLM_ADMIN_READ_ONLY=true`) exposes only reviewed reads. `LITELLM_ADMIN_TOOLS` is a comma allow-list (intersected with read-only).
- Results up to 16 KB come back inline; larger ones go through `read_admin_result` (5-minute snapshots). Tool calls are not retried.
- "The gateway enforces the caller's permissions and feature entitlements." The source calls the management API in-process through an ASGI transport. It computes visible tools per caller with `RouteChecks.should_call_route(route=path, valid_token=auth, request=...)` (`_admin_tool_visible`, `get_admin_mcp_permissions`). [A-adm]
- The docs do not mention `key_type` or `allowed_routes`, but source shows `allowed_routes` affects tool visibility.
- Blog date shown by the fetch was 2026-09-23 (unverified).
- The `/v1/mcp/server` CRUD routes are plain REST for managing MCP server records, not MCP tools.

### A6. Design takeaways for LiteGen (inference)

- Mirror the split: a generation MCP endpoint (`/mcp`) in the "llm_api" class, and a separate admin MCP endpoint (cf. `/admin/mcp`) in the "management" class.
- Wire every MCP tool to the HTTP route it wraps. Run the same route authorizer in `call_tool`. Filter `tools/list` with it, as `admin_mcp.py` does.
- LiteLLM also lets whole planes be disabled by env flags (`DISABLE_LLM_API_ENDPOINTS`, `DISABLE_ADMIN_ENDPOINTS`). [A-typ]
- Keep the owner-role check and key `allowed_routes` as two independent layers (as LiteLLM does).

### Section A sources
- [A-key] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/types/proxy/management_endpoints/key_requests.py
- [A-kme] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/management_endpoints/key_management_endpoints.py
- [A-typ] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/_types.py
- [A-rol] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/types/proxy/auth/user_roles.py
- [A-hdr] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/types/proxy/auth/special_headers.py
- [A-rc] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/auth/route_checks.py
- [A-ac] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/auth/auth_checks.py
- [A-mcpauth] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/mcp_server/auth/user_api_key_auth_mcp.py
- [A-mcpreadme] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/mcp_server/README.md
- [A-adm] https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/proxy/admin_mcp.py
- [A-agents] https://raw.githubusercontent.com/BerriAI/litellm/main/AGENTS.md
- [D-vk] https://docs.litellm.ai/docs/proxy/virtual_keys (anchors `#what-a-key-inherits-from-its-owner`, `#default-keygenerate-params`)
- [D-tok] https://docs.litellm.ai/docs/proxy/token_auth#allowing-other-provider-routes-for-teams
- [D-ac] https://docs.litellm.ai/docs/proxy/access_control (`#global-proxy-roles`, `#organizationteam-specific-roles`, `#team-member-permissions`)
- [D-auth] https://docs.litellm.ai/docs/auth_overview
- [D-mcp] https://docs.litellm.ai/docs/mcp
- [D-ctl] https://docs.litellm.ai/docs/mcp_control
- [D-la] https://docs.litellm.ai/docs/proxy/liteadmin_mcp ; [D-lae] https://docs.litellm.ai/docs/proxy/liteadmin_mcp_enterprise ; [D-blog] https://docs.litellm.ai/blog/litellm-admin-mcp ; [G-la] https://github.com/BerriAI/litellm-admin-mcp

---

## B. MCP specification (current 2026-07-28; previous 2025-11-25)

### B0. What changed between the two revisions

| Topic | 2025-11-25 | 2026-07-28 |
|---|---|---|
| Handshake and sessions | `initialize`; `MCP-Session-Id` optional (404 once terminated) [S-t25] | No `initialize`. Each request carries `_meta` `io.modelcontextprotocol/protocolVersion`, `.../clientCapabilities`, `.../clientInfo`. Mandatory `server/discover`. Server ignores `Mcp-Session-Id`. [S-chg][S-t26] |
| Streams | POST plus GET SSE; resumability via `Last-Event-ID`; disconnect is not cancel [S-t25] | POST only; GET and DELETE return 405; no resumability; closing the response stream means cancellation [S-t26] |
| Notifications of change | `listChanged` plus GET stream | `subscriptions/listen` (opt-in); progress and message stay on the request's response stream [S-chg] |
| Tasks | Experimental core utility [S-task25] | Extension `io.modelcontextprotocol/tasks` (SEP-2663) [S-ext-tasks] |
| Elicitation / sampling | Server-to-client requests (form and url modes) [S-elic] | Only via MRTR `InputRequiredResult`; `elicitationId` and `notifications/elicitation/complete` removed [S-mrtr][S-chg] |
| Headers | `MCP-Protocol-Version` | Plus required `Mcp-Method` (all) and `Mcp-Name` (`tools/call`, `resources/read`, `prompts/get`); `x-mcp-header` maps to `Mcp-Param-{Name}` [S-chg] |
| Caching | none | `ttlMs` and `cacheScope` required on list/read/discover results [S-chg] |
| Client registration | CIMD SHOULD; DCR MAY [S-auth25] | DCR **deprecated** in favour of CIMD; DCR requires `application_type` [S-chg][S-reg26] |
| Auth additions | none | RFC 9207 `iss` validation; credentials bound to issuer; scope union and hierarchies [S-chg][S-auth26] |
| Results | `structuredContent` is a JSON object | Any JSON value; `inputSchema`/`outputSchema` any JSON Schema 2020-12; all results carry `resultType` [S-chg] |
| Deprecated | | Roots, Sampling, Logging (SEP-2577); `ping` and `logging/setLevel` removed; HTTP+SSE transport deprecated [S-chg] |

### B1. Authorization (server acts as an OAuth 2.1 resource server)

Authorization is optional. When used, the MCP server is a resource server. [S-auth25][S-auth26]

- **Protected-resource metadata (RFC 9728).** Servers MUST implement it. The document MUST include `authorization_servers` (at least one). Discovery uses `WWW-Authenticate: Bearer resource_metadata="..."` on 401 and/or the well-known URI `/.well-known/oauth-protected-resource/<mcp-path>` (e.g. `/.well-known/oauth-protected-resource/public/mcp`), with the root `/.well-known/oauth-protected-resource` as fallback. Servers SHOULD include `scope` in the challenge.
- **Status codes.** 401 for invalid or expired tokens. 403 `WWW-Authenticate: Bearer error="insufficient_scope", scope="…", resource_metadata="…", error_description="…"` for step-up. 400 for malformed requests.
  - 2026-07-28 adds: SHOULD list all scopes for the operation in one challenge; clients take the union with prior scopes; servers MUST account for scope hierarchies. [S-auth26]
- **AS metadata.** The AS must offer RFC 8414 or OIDC discovery. Clients try, in order: RFC 8414 path-insertion, OIDC path-insertion, OIDC path-append. The `issuer` must match. [S-asd26]
- **Registration priority.** Pre-registered, then CIMD, then DCR, then prompt the user. [S-auth25][S-reg26]
  - CIMD: the AS advertises `client_id_metadata_document_supported: true`. The `client_id` is an HTTPS URL with a path. The document must contain `client_id`, `client_name` and `redirect_uris`.
  - DCR (RFC 7591) is MAY in 2025-11-25 and deprecated in 2026-07-28.
- **PKCE.** Clients MUST use S256 and MUST refuse if AS metadata lacks `code_challenge_methods_supported`. [S-auth25]
- **Resource indicators (RFC 8707).** `resource` MUST appear in both authorization and token requests, set to the canonical MCP server URI. [S-auth25]
- **Token handling.** Bearer goes in `Authorization` on every request and MUST NOT be in the query string. Servers MUST validate the token was issued for them (audience). They MUST NOT accept other tokens. They MUST NOT pass the client's token through to upstream APIs. [S-auth25][S-sec26]
- **2026-07-28 extras.** RFC 9207 `iss` (AS SHOULD return it; clients MUST validate if present). Servers SHOULD NOT put `offline_access` in scopes. Authorization extensions (client credentials, enterprise-managed) are specified at https://github.com/modelcontextprotocol/ext-auth. [S-auth26]

### B2. What clients actually require

**Claude (claude.ai, Desktop, mobile, Cowork, Code)** `(p)` [C-bld][C-auth][C-rev]
- Streamable HTTP is recommended; legacy HTTP+SSE is being deprecated.
- Claude follows auth specs 2025-03-26, 2025-06-18 and 2025-11-25.
- Auth types listed: `oauth_dcr`, `oauth_cimd` (default-supported), `oauth_anthropic_creds`, `custom_connection` (contact Anthropic), `static_headers` (beta; org Owner enters API key or bearer; header names `authorization`, `x-api-key`, `x-auth-token` work), `none`.
- **CIMD is used only if** AS metadata advertises both `client_id_metadata_document_supported: true` and `"none"` in `token_endpoint_auth_methods_supported`. Otherwise Claude falls back to DCR. DCR registers a new client on every fresh connection.
- A `401` with `WWW-Authenticate: Bearer resource_metadata=...` is required to start sign-in; the header is ignored on a 200.
- PRM `resource` must equal the MCP URL exactly. Claude uses only the first `authorization_servers` entry. It probes `/.well-known/oauth-protected-resource/<path>`, then the root.
- PKCE S256 always. `scope` comes from the challenge, else `scopes_supported`; `offline_access` is appended if listed.
- The token endpoint must accept `application/x-www-form-urlencoded`. Use `invalid_grant` for bad refresh and rotate refresh tokens for public clients.
- Endpoint latency limits: 10 s for discovery, registration and token; 30 s for refresh. No `client_credentials` grant. Egress range `160.79.104.0/21`.
- Callbacks: `https://claude.ai/api/mcp/auth_callback` (hosted); Claude Code uses loopback `http://localhost/callback` or `http://127.0.0.1/callback` (port-agnostic), with CIMD at https://claude.ai/oauth/claude-code-client-metadata.
- No resource subscriptions or sampling.
- Limits: claude.ai/Desktop about 150,000 chars per tool result and 240 s per call.
- Directory review requires each tool to have a `title`, plus `readOnlyHint:true` (read-only) or `destructiveHint:true` (modifying). These set auto-permissions. Tool names must be at most 64 chars, with no catch-all read+write tool.

**Claude Code** `(p)` [CC]
- HTTP is recommended; SSE is deprecated.
- OAuth discovery runs RFC 9728 then RFC 8414. DCR and CIMD are automatic. Flags: `--client-id/--client-secret/--callback-port`; settings `oauth.authServerMetadataUrl` and `oauth.scopes`.
- Static `headers` (an `Authorization` header disables OAuth) and `headersHelper` are supported.
- `list_changed` and elicitation are supported.
- **Tasks are in public beta (v2.1.304+)**: `resultType:"task"`, `pollIntervalMs` clamped to 250-30000, default 2000.
- Progress notifications do not extend `timeout`.
- Output warns above 10,000 tokens; default limit 25,000 (`MAX_MCP_OUTPUT_TOKENS`). Text over 50,000 chars is saved to a file unless the tool sets `_meta["anthropic/maxResultSizeChars"]` (ceiling 500,000).
- Images (PNG, JPEG, GIF, WebP) are shown inline and the original is saved to a file (v2.1.283+).
- `_meta["anthropic/requiresUserInteraction"]: true` forces per-call approval.
- `annotations.idempotentHint` and `readOnlyHint` let Claude Code retry a pending call once and parallelize read-only calls.
- `MCP_TOOL_TIMEOUT` defaults to about 28 h; per-server `timeout` ms; 5-minute idle timeout for HTTP; auto-background after 2 min (`CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS`).
- Tool names become `mcp__server__tool`, and names over 128 chars are dropped.
- Resources are used via `@server:protocol://path`; prompts as `/mcp__server__prompt`.
- The v2 runtime on SDK 2.0 adds protocol 2026-07-28 (`MCP_SDK_GENERATION`, `MCP_PROTOCOL_NEGOTIATION=auto|legacy`).

**ChatGPT / OpenAI** `(p)` [O-auth][O-srv][O-api]
- Auth: PRM (`resource`, `authorization_servers`) via well-known or WWW-Authenticate. AS metadata must list S256 in `code_challenge_methods_supported`.
- **CIMD is preferred** (token auth `none` or `private_key_jwt`). DCR is optional and supported. Predefined clients are listed. PKCE S256. `resource` is sent on authorization and token requests (copy it to `aud`).
- Redirect URIs: `https://chatgpt.com/connector/oauth/{callback_id}`, or `https://chatgpt.com/connector_platform_oauth_redirect` when RFC 9207 `iss` is supported.
- No client-credentials grant. mTLS and egress IPs are documented.
- Per-tool `securitySchemes` in `_meta` (`noauth`/`oauth2`), and `_meta["mcp/www_authenticate"]` for in-tool account linking.
- Transport: Streamable HTTP at a stable URL, typically ending `/mcp`. The page links spec 2025-06-18; the negotiated version is unverified.
- Annotations `readOnlyHint`, `destructiveHint` and `openWorldHint` drive confirmation behaviour. `structuredContent`, `outputSchema` and `_meta` are documented.
- For deep research and company knowledge, `search` and `fetch` tools are expected. Write actions require manual confirmation (stated on the API MCP page).
- Image and `resource_link` handling, size limits and timeouts are not stated.

**VS Code** `(p)` [V]: tools, resources (via "MCP: Browse Resources"), prompts, MCP Apps (experimental), stdio and HTTP/SSE. OAuth, DCR/CIMD, elicitation, sampling, `list_changed`, tasks and spec versions are not mentioned on that page.

**Cursor** `(p)` [CU]
- Transports: stdio, SSE and Streamable HTTP.
- OAuth: static client (`auth` with `CLIENT_ID`, optional `CLIENT_SECRET`, `scopes`), auth-code plus PKCE, reads PRM (RFC 9728), uses resource indicators (RFC 8707), and DCR as a public client.
- Redirects: `https://www.cursor.com/agents/mcp/oauth/callback` and `http://localhost:8787/callback`.
- Features: Tools, Prompts, Resources, Roots, Elicitation, Apps. Images are attached as base64.
- CIMD, tasks, `list_changed`, structured content, annotations and tool cap are not mentioned.

**Codex** `(p)` [CX]
- Transports: STDIO and Streamable HTTP.
- Auth: bearer via `bearer_token_env_var`, `http_headers`, `env_http_headers`, `http_headers_helper`. OAuth with CIMD or DCR when available (`codex mcp login`, `--oauth-client-id`, `--oauth-client-registration cimd|dcr`).
- `startup_timeout_sec` defaults to 10. **`tool_timeout_sec` defaults to 60.**
- It supports `enabled_tools`/`disabled_tools`, approval modes and `output_token_limit`, and reads server `instructions`.
- Resources, prompts, elicitation, tasks, `list_changed`, structured content, images and spec versions are not mentioned.

**Client matrix (Tasks and Apps, community-maintained)** [S-mat]
- Tasks are supported by Claude (web), Claude Code and MCPJam only. Not Claude Desktop, VS Code Copilot, ChatGPT or Cursor. Codex is not listed.
- MCP Apps are supported by Claude web and Desktop, VS Code, ChatGPT, Cursor and others.
- Which Tasks flavour Claude web implements is unverified. Claude Code's docs describe the 2026-07-28 extension shape (`resultType:"task"`, `pollIntervalMs`).
- Which protocol revision claude.ai, ChatGPT, Cursor, VS Code and Codex negotiate is unverified.

### B3. Server features for long-running jobs and binary outputs

| Feature | Spec | Client support (verified only) |
|---|---|---|
| Progress | Client sends `params._meta.progressToken`. Server MAY send `notifications/progress {progressToken, progress (must increase), total?, message?}`, delivered on the request's SSE stream, rate-limited and stopped after completion. [S-prog25][S-prog26] | Claude Code accepts but does not extend timeouts `(p)` [CC]. Others unverified. |
| Tasks | 2025-11-25: capability `tasks.{list,cancel,requests.tools.call}`, `params.task:{ttl}`, `CreateTaskResult`, `tasks/get\|result\|list\|cancel`, `notifications/tasks/status`, statuses working/input_required/completed/failed/cancelled, bound to auth context. [S-task25] 2026-07-28 extension: client opts in per request via `_meta."io.modelcontextprotocol/clientCapabilities".extensions`; server decides per request to return `resultType:"task"` with a flat `Task{taskId,status,statusMessage?,createdAt,lastUpdatedAt,ttlMs\|null,pollIntervalMs?}`; `tasks/get` returns `DetailedTask`; `tasks/update{taskId,inputResponses}`; `tasks/cancel` is cooperative; no `tasks/list`; never return a task to a client that did not declare support. [S-ext-tasks] | Claude web and Claude Code only (matrix above). Claude Code beta v2.1.304+. |
| `resource_link` | Content type `resource_link` carrying `ResourceLink{uri,name,title?,description?,mimeType?,size?,icons?,annotations?}`. [S-tools25] | Client handling unverified for all clients. |
| Image content | `{type:"image", data (base64), mimeType}` plus text, audio, `resource_link` and embedded `resource`. [S-tools25] | Claude Code inline plus saved original `(p)` [CC]. Cursor attaches base64 `(p)` [CU]. Others unverified. |
| `structuredContent` + `outputSchema` | Tool SHOULD also return serialized JSON in a TextContent block. If `outputSchema` is given, the server MUST conform and clients SHOULD validate. 2026-07-28 allows any JSON value. [S-tools25][S-tools26] | ChatGPT documents both `(p)` [O-srv][O-api]. Others unverified. |
| Annotations | `title?`, `readOnlyHint` (default false), `destructiveHint` (default true, meaningful when not read-only), `idempotentHint` (default false), `openWorldHint` (default true). Untrusted unless the server is trusted. [S-tools25] | Claude directory requires them; Claude Code and ChatGPT use them (see B2). |
| `title` / `icons` | Tool `title` and `icons: Icon{src,mimeType?,sizes?,theme?}`. [S-tools25] | Claude directory requires `title` `(p)`. Icons unverified. |
| Errors | Protocol errors (JSON-RPC, e.g. -32602) vs tool execution errors `isError:true`. Input validation errors belong in `isError`. Tool names are 1-128 chars `[A-Za-z0-9_.-]`. [S-tools25] | |
| Elicitation | 2025-11-25: `form` (flat primitive schema, MUST NOT request secrets) and `url` modes, `URLElicitationRequiredError` -32042, actions accept/decline/cancel. 2026-07-28: only via MRTR (`inputRequests`, `requestState` integrity-protected, retry with `inputResponses`). [S-elic][S-mrtr] | Claude Code and Cursor support elicitation `(p)`. Others unverified. |
| `list_changed` | `listChanged` capability (2025-11-25); `subscriptions/listen` (2026-07-28). 2026-07-28 `tools/list` may vary by authorization presented but MUST NOT vary per connection. [S-tools26] | Claude Code `(p)`. Others unverified. |
| Pagination | Opaque cursors, `nextCursor`; invalid cursor returns -32602; list ordering SHOULD be deterministic. [S-page] | Per-client support unverified. |
| Discover | `{"resultType":"complete","supportedVersions":[…],"capabilities":{…},"_meta":{".../serverInfo":{…}},"instructions":"…","ttlMs":…,"cacheScope":"public"}` [S-disc] | |

### B4. Streamable HTTP details

**2026-07-28** [S-t26]
- One endpoint; every JSON-RPC message is its own POST. `Accept` must list `application/json` and `text/event-stream`.
- The server MUST answer a request with either `application/json` or `text/event-stream`; clients must handle both. Notifications get 202.
- SHOULD send `X-Accel-Buffering: no` on SSE.
- GET and DELETE return 405. Ignore `Mcp-Session-Id` and `Last-Event-ID` from old clients.
- `MCP-Protocol-Version` is required and must match `_meta`; unsupported gives 400 `UnsupportedProtocolVersionError`. Unknown method gives 404 plus -32601.
- **Origin:** "Servers MUST validate the Origin header on all incoming connections... If present and invalid, MUST respond 403". SHOULD authenticate all connections; bind localhost for local servers.

**2025-11-25** [S-t25]
- POST plus GET on one endpoint. `MCP-Session-Id` is optional (400 if missing when required, 404 after termination, DELETE to end, 405 allowed).
- Resumability via event IDs and `Last-Event-ID`.
- `MCP-Protocol-Version` on subsequent requests. Absent means assume `2025-03-26`; invalid gives 400.
- Origin validation gives 403. A disconnect is not a cancel.

### Section B sources
- [S-ver] https://modelcontextprotocol.io/specification/versioning ; index https://modelcontextprotocol.io/llms.txt
- [S-chg] https://modelcontextprotocol.io/specification/2026-07-28/changelog.md
- [S-t26] https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http.md
- [S-t25] https://modelcontextprotocol.io/specification/2025-11-25/basic/transports.md
- [S-auth25] https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
- [S-auth26] https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/index.md ; [S-asd26] …/authorization-server-discovery.md ; [S-reg26] …/client-registration.md ; [S-sec26] …/security-considerations.md
- [S-tools25] https://modelcontextprotocol.io/specification/2025-11-25/server/tools.md ; [S-tools26] https://modelcontextprotocol.io/specification/2026-07-28/server/tools.md
- [S-prog25] https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/progress.md ; [S-prog26] https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/progress.md
- [S-task25] https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks.md
- [S-ext-tasks] https://modelcontextprotocol.io/extensions/tasks/overview.md ; https://github.com/modelcontextprotocol/ext-tasks (`schema/2026-07-28/schema.ts`)
- [S-elic] https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation.md ; [S-mrtr] https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/mrtr.md
- [S-page] https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/pagination.md ; [S-disc] https://modelcontextprotocol.io/specification/2026-07-28/server/discover.md
- [S-mat] https://raw.githubusercontent.com/modelcontextprotocol/modelcontextprotocol/main/docs/extensions/client-matrix.mdx (rendered: https://modelcontextprotocol.io/extensions/client-matrix.md)
- [C-bld] https://claude.com/docs/connectors/building ; [C-auth] https://claude.com/docs/connectors/building/authentication.md ; [C-rev] https://claude.com/docs/connectors/building/review-criteria.md
- [CC] https://code.claude.com/docs/en/mcp
- [O-auth] https://developers.openai.com/apps-sdk/build/auth ; [O-srv] https://developers.openai.com/plugins/build/mcp-server ; [O-api] https://developers.openai.com/api/docs/mcp
- [V] https://code.visualstudio.com/docs/copilot/customization/mcp-servers ; [CU] https://cursor.com/docs/context/mcp
- [CX] https://learn.chatgpt.com/docs/extend/mcp?surface=cli (redirect from https://developers.openai.com/codex/mcp)

---

## C. Rust SDK `rmcp`

`RS` = `https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.8.0`. All source paths below are under `RS/`. Docs home: https://docs.rs/rmcp/latest/rmcp/. I read signatures from the tag source; item-level docs.rs pages were not fetched.

### C1. Version and MSRV
- Newest stable is **3.8.0**, published 2026-10-26T22:03:56Z. Earlier: 3.7.0 (2026-10-22), 3.6.0 (2026-10-20), 3.5.1 (2026-10-05). [crates.io: https://crates.io/api/v1/crates/rmcp]
- **MSRV is `rust-version = "1.88"`, `edition = "2024"`.** [RS/Cargo.toml]
- docs.rs home lists modules `handler`, `model`, `service`, `task_manager` and `transport`. `task_manager` is described as the "Server-side runtime for the MCP Tasks extension (SEP-2663)". [docs.rs home]
- The 3.x API differs from older 0.x memory:
  - `get_info()` returns `ServerConfig`.
  - `call_tool` returns `CallToolResponse`.
  - The session setting is `legacy_session_mode`. No `stateful_mode` field exists in 3.8.0.

### C2. Feature flags [RS/crates/rmcp/Cargo.toml]
```toml
default = ["base64","macros","server"]
server  = ["transport-async-rw","schemars","dep:pastey","uuid"]
macros  = ["dep:rmcp-macros","dep:pastey"]
elicitation = ["dep:url"]
request-state                    # SEP-2322 HMAC requestState sealing
server-side-http = ["uuid","dep:rand","dep:tokio-stream","dep:http","dep:http-body","dep:http-body-util","dep:bytes","dep:sse-stream","tower","base64"]
transport-streamable-http-server = ["transport-streamable-http-server-session","server-side-http","transport-worker"]
transport-streamable-http-server-session = ["transport-async-rw","dep:async-trait","dep:tokio-stream"]
auth = ["dep:async-trait","dep:oauth2","__reqwest","dep:url"]   # client-side OAuth
```
- A minimal axum 0.8 server needs `server` (default) plus `transport-streamable-http-server`. Add `macros` only for `#[tool]`, and `elicitation` only for `peer.elicit*`. This is derived from the feature graph and was not compiled here.
- rmcp's dev-dependency is `axum = "0.8"`.
- Do not enable `local`. It drops the `StreamableHttpService` export (`#[cfg(all(feature = "transport-streamable-http-server", not(feature = "local")))]`, RS/crates/rmcp/src/transport.rs:113-115) and makes `ServerHandler` non-`Send`.

### C3. `StreamableHttpService`, config, session managers, mounting [RS/crates/rmcp/src/transport/streamable_http_server/tower.rs]
```rust
// tower.rs:1408 — impl<S, M> StreamableHttpService<S, M> where S: ServerHandler + Send + 'static, M: SessionManager
pub fn new(
    service_factory: impl Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
    session_manager: Arc<M>,
    config: StreamableHttpServerConfig,
) -> Self
pub async fn handle<B>(&self, request: Request<B>) -> Response<BoxBody<Bytes, Infallible>>   // tower.rs:1806
// tower.rs:1354: impl tower_service::Service<Request<RequestBody>> — Response = BoxResponse, Error = Infallible
```
`StreamableHttpServerConfig` is `#[non_exhaustive]` and `Clone`; build it with `Default` plus `with_*` builders (tower.rs:62-300). The `session_store` field is `pub`, but no `with_session_store` builder exists.

| Field | Default | Meaning |
|---|---|---|
| `sse_keep_alive: Option<Duration>` | `Some(15s)` | SSE ping interval |
| `sse_retry: Option<Duration>` | `Some(3s)` | SSE priming retry |
| **`legacy_session_mode: bool`** | `true` | Applies only to protocol `< 2026-07-28`. "requests negotiating that version are always served statelessly regardless of this setting". |
| **`json_response: bool`** | `false` | "When true and `legacy_session_mode` is false, the server prefers `Content-Type: application/json` ... If the handler emits a notification or request before the final response, the server falls back to `text/event-stream`" |
| `cancellation_token: CancellationToken` | new | Cancelling it terminates sessions and stops accepting requests |
| **`allowed_hosts: Vec<String>`** | `["localhost","127.0.0.1","::1"]` | `Host` check, 403 "Forbidden: Host header is not allowed". "Public deployments should override this". An **empty list allows all hosts** (tower.rs:959-963). |
| **`allowed_origins: Vec<String>`** | empty | Empty disables Origin validation. Non-empty enables it (missing `Origin` still passes; entries need a scheme; `:*` for any port; a portless entry currently matches any port, deprecated). 403 "Forbidden: Origin header is not allowed". |
| `session_store: Option<Arc<dyn SessionStore>>` | `None` | Cross-instance session restore |
| `max_request_body_bytes: usize` | 4 MiB | 413 when exceeded |
| `stateless_protocol_metadata_required: bool` | `false` | Require per-request protocol metadata |

Builders: `with_allowed_hosts(impl IntoIterator<Item = impl Into<String>>)`, `disable_allowed_hosts()`, `with_allowed_origins(...)`, `enforce_origin_validation()`, `disable_allowed_origins()`, `with_sse_keep_alive(Option<Duration>)`, `with_sse_retry(Option<Duration>)`, `with_legacy_session_mode(bool)`, `with_json_response(bool)`, `with_cancellation_token(CancellationToken)`, `with_max_request_body_bytes(usize)`, `with_stateless_protocol_metadata_required(bool)`.

Request handling:
- Method dispatch (tower.rs:1817-1828): POST always. GET only if `legacy_session_mode` or an event store exists. DELETE only if `legacy_session_mode`. Otherwise 405 with `Allow: POST` (or `GET, POST` or `GET, POST, DELETE`).
- POST (tower.rs:1983-2015): `Accept` must contain both `application/json` and `text/event-stream`, else 406. `Content-Type` must start with `application/json`, else 415.
- `legacy_session_mode=false` (tower.rs:2029, 2255-2345): every POST is served by a one-shot service, and no `Mcp-Session-Id` is issued.
  - A stateless server rejects JSON-RPC responses ("stateless server does not accept JSON-RPC responses", tower.rs:2344-2348). So server-initiated requests such as `peer.elicit*` cannot round-trip statelessly (inference).
- `MCP-Protocol-Version` is validated against `ProtocolVersion::KNOWN_VERSIONS` (400 if unknown). Absent means `2025-03-26`.

Session managers [RS/crates/rmcp/src/transport/streamable_http_server/session.rs, session/local.rs, session/never.rs, session/store.rs]:
- `session::local::LocalSessionManager` is the in-memory default (`LocalSessionManager::default().into()`), with `with_event_store(Arc<dyn EventStore>)`.
- `session::never::NeverSessionManager` is `#[derive(Debug, Clone, Default)]`. Doc: "disables sessions entirely (stateless mode)"; also `with_event_store(...)`.
- `SessionStore` and `EventStore` traits are in `store.rs`. `SessionManager` is the trait for custom managers (database or Redis).

Mount (official examples `examples/servers/src/counter_streamhttp.rs` and `simple_auth_streamhttp.rs`):
```rust
let service = StreamableHttpService::new(
    || Ok(Counter::new()),
    LocalSessionManager::default().into(),
    StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
);
let router = axum::Router::new().nest_service("/mcp", service);
// auth example:
Router::new().nest_service("/mcp", mcp_service)
    .layer(middleware::from_fn_with_state(token_store, auth_middleware))
```
Stateless variant (inference from the verified API; not compiled): pass `Arc::new(NeverSessionManager::default())` with `.with_legacy_session_mode(false).with_json_response(true).with_allowed_hosts([...])`.

### C4. Reading the HTTP request inside a handler
From the `StreamableHttpService` docs (tower.rs:1274-1334): "The service consumes the request body but injects the remaining [`http::request::Parts`] into [`crate::model::Extensions`], which is accessible through [`crate::service::RequestContext`]." The injection sites are tower.rs:1538 (stateless) and 2069, 2154 and 2275 (other paths).
```rust
// service.rs:1194
pub struct RequestContext<R: ServiceRole> {
    pub ct: CancellationToken, pub id: RequestId, pub meta: RequestMetaObject,
    pub extensions: Extensions, pub peer: Peer<R>,
}
// documented usage (tower.rs:1320-1330)
let parts = ctx.extensions.get::<http::request::Parts>().unwrap();
let state = parts.extensions.get::<AppState>().unwrap();   // axum Extension / middleware-inserted state
parts.headers.get("mcp-session-id")
```
- Axum middleware layered on the `nest_service` router can insert an auth context into request extensions before rmcp runs. It then appears in `Parts.extensions`.
- The official manual handler reads `context.extensions.get::<axum::http::request::Parts>()` (RS/examples/servers/src/common/counter.rs:~273).
- Macro handlers can use `Extension(parts): Extension<Parts>`.

### C5. Manual `ServerHandler` (no macros)
`pub trait ServerHandler: Sized + Send + Sync + 'static` (handler/server.rs:642). Methods are `async fn`-compatible, returning `impl Future<Output = …> + MaybeSendFuture + '_`. `McpError` is `rmcp::ErrorData`. [RS/crates/rmcp/src/handler/server.rs]
```rust
fn get_info(&self) -> ServerConfig                                                    // :602
fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]>              // :388 default = KNOWN_VERSIONS
fn discover(&self, context: RequestContext<RoleServer>) -> Result<DiscoverResult, McpError>   // :392
fn list_tools(&self, request: Option<PaginatedRequestParams>, context: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError>   // :538
fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError>           // :531
fn get_tool(&self, _name: &str) -> Option<Tool>                                       // :549 default None => "bypasses validation"
fn list_resources(&self, request: Option<PaginatedRequestParams>, context: RequestContext<RoleServer>) -> Result<ListResourcesResult, McpError>   // :429
fn list_resource_templates(&self, request: Option<PaginatedRequestParams>, context: RequestContext<RoleServer>) -> Result<ListResourceTemplatesResult, McpError> // :436
fn read_resource(&self, request: ReadResourceRequestParams, context: RequestContext<RoleServer>) -> Result<ReadResourceResponse, McpError>   // :445
fn get_task(&self, request: GetTaskParams, context: RequestContext<RoleServer>) -> Result<GetTaskResult, McpError>   // :607
fn update_task(&self, request: UpdateTaskParams, context: RequestContext<RoleServer>) -> Result<(), McpError>        // :618
fn cancel_task(&self, request: CancelTaskParams, context: RequestContext<RoleServer>) -> Result<(), McpError>        // :629
fn accepted_subscription_filter(&self, &SubscriptionFilter) -> Option<SubscriptionFilter>; fn listen(&self, SubscriptionContext)   // :460/:472, 2026-07-28 subscriptions/listen
```
Other hooks: `ping`, `initialize`, `negotiate_initialize`, `complete`, `set_level`, `get_prompt`, `list_prompts`, `on_custom_request`, `on_cancelled`, `on_progress`, `on_initialized`, `on_roots_list_changed`, `on_custom_notification`.

Defaults: `list_*` return empty results; `call_tool` and `read_resource` return `method_not_found`.

Guidance in the `call_tool` docs: tool failures go in `Ok(CallToolResult::error(vec![ContentBlock::text(..)]))`. Reserve `Err(ErrorData)` for unroutable requests; clients render protocol errors opaquely ("the caller will not see your message").

Types [RS/crates/rmcp/src/model.rs]:
- `ServerConfig = InitializeResult` (model.rs:1341). Builders: `ServerConfig::new(ServerCapabilities)`, `.with_instructions(..)`, `.with_server_info(Implementation)`, `.with_protocol_version(..)`. `ServerInfo` is a deprecated alias.
- `Implementation { name, title?, version, description?, icons?, website_url? }` (model.rs:1687).
- `PaginatedRequestParams { meta: Option<RequestMetaObject>, cursor: Option<String> }` (model.rs:1759).
- `ListToolsResult`, `ListResourcesResult`, `ListResourceTemplatesResult` are generated by `paginated_result!` (model.rs:1892). Fields: `result_type`, `meta`, `next_cursor: Option<Cursor>`, `ttl_ms: Option<u64>`, `cache_scope: Option<CacheScope>`, plus `tools: Vec<Tool>` / `resources: Vec<Resource>` / `resource_templates: Vec<ResourceTemplate>`. Constructors `with_all_items(items)`, `.with_ttl_ms(u64)`, `.with_cache_scope(..)`, `Default`.
- `ReadResourceRequestParams { meta, uri: String, input_responses?, request_state? }` with `::new(uri)` (model.rs:1999).
- `CallToolRequestParams { meta, name: Cow<'static,str>, arguments: Option<JsonObject>, input_responses?, request_state? }` (model.rs:4409).
- `ServerCapabilities::builder()` with `.enable_tools()`, `.enable_tool_list_changed()`, `.enable_resources()`, `.enable_resources_list_changed()`, `.enable_resources_subscribe()`, `.enable_prompts()`, `.enable_tasks()`, `.build()` [RS/crates/rmcp/src/model/capabilities.rs].

Official manual resource handler (RS/examples/servers/src/common/counter.rs:208-250): `ServerConfig::new(ServerCapabilities::builder().enable_prompts().enable_resources().enable_tools().build()).with_server_info(Implementation::from_build_env()).with_instructions(..)`; `list_resources` returns `ListResourcesResult { resources: vec![Resource::new(uri, name)…], ..Default::default() }`; `read_resource` returns `Ok(ReadResourceResult::new(vec![ResourceContents::text(text, uri.clone())]).into())`, with `Err(McpError::resource_not_found("resource_not_found", Some(json!({"uri": uri}))))` for unknown URIs.

Illustrative skeleton (assembled from the signatures above; not compiled here):
```rust
impl ServerHandler for LiteGenMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(Implementation::new("litegen", env!("CARGO_PKG_VERSION")))
    }
    async fn list_tools(&self, _r: Option<PaginatedRequestParams>, ctx: RequestContext<RoleServer>)
        -> Result<ListToolsResult, McpError> {
        let parts = ctx.extensions.get::<http::request::Parts>();   // then parts.extensions.get::<AuthCtx>()
        Ok(ListToolsResult::with_all_items(visible_tools))
    }
    async fn call_tool(&self, req: CallToolRequestParams, ctx: RequestContext<RoleServer>)
        -> Result<CallToolResponse, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("ok")]).into())
    }
}
```

### C6. Protocol versions, tasks, progress, content types, structured output, annotations

**Versions.** [RS/crates/rmcp/src/model.rs:167-207]
- Constants: `V_2024_11_05`, `V_2025_03_26`, `V_2025_06_18`, `V_2025_11_25`, `V_2026_07_28`.
- `LATEST = V_2026_07_28`; `LATEST_WITH_INITIALIZE = V_2025_11_25`; `KNOWN_VERSIONS` lists all five; `known_up_to(&max)`; `has_initialize()`.
- Narrow with `fn supported_protocol_versions(&self) -> Cow<'static,[ProtocolVersion]> { Cow::Borrowed(ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25)) }`.

**Responses.** [RS/crates/rmcp/src/model/mrtr.rs]
- `#[non_exhaustive] pub enum CallToolResponse { Complete(CallToolResult), InputRequired(InputRequiredResult), Task(CreateTaskResult) }`, with `From<…>` for each variant.
- Similar `GetPromptResponse` and `ReadResourceResponse` enums.

**Tasks (SEP-2663 only).**
- Symbols: `TASKS_EXTENSION_ID = "io.modelcontextprotocol/tasks"`, `Task`, `DetailedTask`, `CreateTaskResult`, `GetTaskResult`, `TaskStatus`.
- `ClientCapabilities::supports_tasks()` (capabilities.rs:198).
- Server side: `rmcp::task_manager::{TaskManager, TaskOptions, TaskContext, TaskExit}` [RS/crates/rmcp/src/task_manager.rs]:
```rust
pub fn spawn<F>(&self, options: TaskOptions, make_future: F) -> Task
  where F: FnOnce(TaskContext) -> TaskFuture      // TaskFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, TaskExit>> + Send>>   (:315)
pub fn get_task(&self, task_id: &str) -> Result<DetailedTask, McpError>   // :402
pub fn update_task(..) // :416     pub fn cancel_task(&self, task_id: &str) -> Result<(), McpError>   // :453
// TaskOptions::new().with_ttl_ms(..).with_poll_interval_ms(..).with_status_message(..); DEFAULT_TASK_TTL_MS = 300_000; DEFAULT_POLL_INTERVAL_MS = 1_000
```
- Official pattern (RS/examples/servers/src/common/task_demo.rs):
  - Check `context.client_capabilities().is_some_and(|caps| caps.supports_tasks())`.
  - Then `let task = self.tasks.spawn(TaskOptions::default(), move |ctx| Box::pin(async move { … Ok(CallToolResult::success(..)) }));` and `return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));`.
  - Delegate `get_task` with `Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))`.
  - Advertise with `ServerCapabilities::builder().enable_tasks()`. Cancellation is cooperative (`ctx.cancelled()`).
- The manager holds state in memory and has no background sweeper; the sweep is opportunistic on task creation (task_manager.rs ~L295-345).
- **2025-11-25 core experimental Tasks are not implemented.** I found no `tasks/list`, `tasks/result`, `params.task` or `execution.taskSupport` symbols in `model.rs`, `model/task.rs`, `model/tool.rs` or `handler/server.rs`. `Tool` has no `execution` field. (Absence-based inference.)

**Progress.** [RS/crates/rmcp/src/model/meta.rs:436, model.rs:1795, service/server.rs ~L940; example RS/examples/servers/src/common/progress_demo.rs]
```rust
// RequestMetaObject
pub fn get_progress_token(&self) -> Option<ProgressToken>
// ProgressNotificationParam (#[non_exhaustive]): progress_token: ProgressToken, progress: f64, total: Option<f64>, message: Option<String>, meta: Option<NotificationMetaObject>
ProgressNotificationParam::new(progress_token: ProgressToken, progress: f64) -> Self   // .with_total(f64) .with_message(..)
// Peer<RoleServer> — generated by method!(peer_not notify_progress ProgressNotification(ProgressNotificationParam))
pub async fn notify_progress(&self, params: ProgressNotificationParam) -> Result<(), ServiceError>
// usage from call_tool
let Some(token) = ctx.meta.get_progress_token() else { /* client did not ask for progress */ };
ctx.peer.notify_progress(ProgressNotificationParam::new(token, 3.0).with_total(10.0).with_message("rendering")).await?;
```
Other `Peer<RoleServer>` methods: `notify_logging_message`, `notify_cancelled`, `notify_resource_updated`, `notify_resource_list_changed`, `notify_tool_list_changed`, `notify_prompt_list_changed`, `create_message` (sampling), and with the `elicitation` feature `elicit::<T>(message)`, `elicit_with_timeout`, `elicit_url`, `elicit_url_with_timeout`, `supported_elicitation_modes()`.

In stateless JSON mode the transport falls back to SSE if the handler emits a notification before the final response (tower.rs ~L2300).

**Content, structured output, annotations, icons.** [RS/crates/rmcp/src/model/content.rs, resource.rs, tool.rs]
- `CallToolResult { result_type: Option<ResultType>, content: Vec<ContentBlock>, structured_content: Option<Value>, is_error: Option<bool>, meta: Option<MetaObject> }`. Constructors `success(Vec<ContentBlock>)`, `error(Vec<ContentBlock>)`, `structured(Value)`, a structured-error variant, and `with_meta`.
- `ContentBlock` (`#[serde(tag="type", rename_all="snake_case")]`): `Text`, `Image`, `Audio`, `Resource(EmbeddedResource)`, `ResourceLink(Resource)`. Constructors `ContentBlock::text(..)`, `::image(data_base64, mime_type)`, `::audio(..)`, `::resource(ResourceContents)`, `::embedded_text(uri, text)`, `::resource_link(Resource)`, `::json(..)`.
- `Resource { uri, name, title?, description?, mime_type?, size: Option<u64>, icons?, meta?, annotations? }` with `Resource::new(uri, name)` and `.with_title/.with_description/.with_mime_type/.with_size/.with_icons/.with_meta/.with_annotations`.
- `ResourceTemplate::new(uri_template, name)`; `ResourceContents::text(text, uri)` and `::blob(blob_base64, uri)`.
- `Tool { name: Cow<'static,str>, title: Option<String>, description: Option<Cow<'static,str>>, input_schema: Arc<JsonObject>, output_schema: Option<Arc<JsonObject>>, annotations: Option<ToolAnnotations>, icons: Option<Vec<Icon>>, meta: Option<MetaObject> }`.
  - `Tool::new(name, description, input_schema)`, `.with_title`, `.with_annotations`, `.with_icons`, `.with_meta`, `.with_raw_output_schema`, `.with_output_schema::<T: JsonSchema>()`, `.with_input_schema::<T>()`.
- `ToolAnnotations { title, read_only_hint, destructive_hint, idempotent_hint, open_world_hint: Option<…> }` with builders `.read_only(bool)`, `.destructive(bool)`, `.idempotent(bool)`, `.open_world(bool)` and `ToolAnnotations::with_title(..)`.
- Example for structured output: RS/examples/servers/src/structured_output.rs.

**Caches / MRTR.** List and read results carry `ttl_ms`/`cache_scope`. MRTR is `CallToolResponse::InputRequired`, with the `request-state` feature for sealing `requestState` (examples `mrtr.rs`, `subscriptions_streamhttp.rs`).

### C7. OAuth resource-server helpers: none server-side
- `rmcp::transport::auth` (feature `auth`) is client-side only: `AuthorizationManager`, `AuthClient`, `CredentialStore`, `WWWAuthenticateParams`, etc. [RS/crates/rmcp/src/transport.rs:85-105, RS/crates/rmcp/src/transport/auth.rs]
- The PRM parse type `ResourceServerMetadata` is private (`struct ResourceServerMetadata` at auth.rs:743, no `pub`).
- Server-side auth is hand-rolled. Official examples:
  - `simple_auth_streamhttp.rs`: bearer-token middleware in front of `nest_service`.
  - `complex_auth_streamhttp.rs`: `.route_layer(...)`, a PRM route at `/.well-known/oauth-protected-resource/mcp` returning `{resource, authorization_servers, scopes_supported, bearer_methods_supported}`, an AS-metadata route, and `WWW-Authenticate: Bearer resource_metadata="..."` on 401.
  - `cimd_auth_streamhttp.rs` also exists.

### C8. Gotchas
1. Public deployments must set `allowed_hosts` or every request gets 403 "Host header is not allowed". Verify what Host header your reverse proxy forwards; `X-Forwarded-Host` handling is unverified.
2. Set `allowed_origins`; the spec says servers MUST validate Origin, but rmcp's default does not.
3. `legacy_session_mode` defaults to `true` (sessions, GET and DELETE enabled). Set `false` for a stateless gateway.
4. Clients must send `Accept: application/json, text/event-stream` (else 406) and `Content-Type: application/json` (else 415).
5. No PRM or resource-server helper exists, so add the well-known route and the 401/403 `WWW-Authenticate` challenges in axum middleware.
6. `get_tool` also feeds SEP-2243 standard-header validation (`validate_standard_headers`, tower.rs ~L2062); the default `None` skips it.
7. Not compiled or run here; rmcp's own tests cover stateless protocol, 2026-07-28 discover, standard headers, MRTR, tasks, session store and event store.

### Section C sources
- crates.io: https://crates.io/api/v1/crates/rmcp
- docs.rs home: https://docs.rs/rmcp/latest/rmcp/
- Repo: https://github.com/modelcontextprotocol/rust-sdk (tag `rmcp-v3.8.0`; raw base https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/rmcp-v3.8.0/)
- Files under `crates/rmcp/`: `Cargo.toml`, `src/transport.rs`, `src/transport/streamable_http_server/{tower.rs,session.rs}`, `session/{local.rs,never.rs,store.rs}`, `src/transport/auth.rs`, `src/handler/server.rs`, `src/model.rs`, `src/model/{meta,capabilities,content,resource,tool,task,mrtr}.rs`, `src/service.rs`, `src/service/server.rs`, `src/task_manager.rs`. Workspace `Cargo.toml` at the repo root.
- Examples under `examples/servers/src/`: `counter_streamhttp.rs`, `simple_auth_streamhttp.rs`, `complex_auth_streamhttp.rs`, `cimd_auth_streamhttp.rs`, `common/counter.rs`, `common/progress_demo.rs`, `common/task_demo.rs`, `structured_output.rs`, `mrtr.rs`, `subscriptions_streamhttp.rs`.

---

## Cross-cutting implications for LiteGen (inference, not source statements)

1. **Two MCP endpoints.**
   - `/mcp` for generation tools and job/asset resources.
   - A separate admin MCP endpoint for the management API, role-gated.
   - Route each tool through the same authorizer as the HTTP route it wraps.
   - Filter `tools/list` by that authorizer. 2026-07-28 allows `tools/list` to vary by authorization presented. [S-tools26]
2. **Auth.**
   - A bearer virtual key works for Claude Code, Codex, Cursor and VS Code (static headers).
   - claude.ai and ChatGPT need OAuth: PRM, a 401 challenge, and an AS (own or external) supporting CIMD and/or DCR with S256, `resource` and audience binding.
   - For Claude's CIMD path the AS must advertise `client_id_metadata_document_supported: true` and `"none"` as a token auth method.
3. **rmcp wiring.**
   - Auth middleware inserts a context, then `nest_service("/mcp", …)`.
   - Read the context from `ctx.extensions.get::<http::request::Parts>()`.
   - Set `legacy_session_mode(false)`, `json_response(true)`, `allowed_hosts` and `allowed_origins`.
4. **Long jobs.**
   - Use `TaskManager` (SEP-2663) when the client declares tasks (Claude web and Claude Code).
   - Provide submit/get_status tools for everything else (Codex 60 s default, ChatGPT, Cursor and VS Code lack Tasks per the matrix).
5. **Binary outputs.**
   - Prefer `resource_link` or signed URLs plus small previews.
   - claude.ai/Desktop cap tool results at about 150k chars. Claude Code defaults to 25k tokens, with images handled separately.
   - Client handling of `resource_link` is unverified.

## Unverified / open items
- LiteLLM: how the code distinguishes omitted vs explicit `key_type=default`; whether a `management` key is truly blocked from `/mcp` calls (inferred only); blog date of LiteAdmin.
- Clients: protocol revision negotiated by claude.ai/Desktop, ChatGPT, Cursor, VS Code, Codex; Claude web's Tasks flavour; `resource_link`, `structuredContent`, icons, pagination, elicitation and `list_changed` support in clients not listed above.
- rmcp: `X-Forwarded-Host` handling; behaviour of `LocalSessionManager` internals (only the surface was read); item-level docs.rs pages not fetched; nothing was compiled.
