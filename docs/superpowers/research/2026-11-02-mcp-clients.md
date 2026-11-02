# MCP clients: setup, auth and limits (research, 2026-11-02)

Research notes for the LiteGen MCP server docs
([design](../specs/2026-11-02-mcp-server-design.md)). Gathered from the
clients' official docs on 2026-11-02; "unverified" means no official source
confirmed it. Placeholders: `https://litegen.example.com/mcp`,
`LITEGEN_API_KEY`. Re-check each client's page when writing its doc: these
products change monthly.

**Moves since 2025:** Anthropic connector docs live at
`claude.com/docs/connectors/*`, Claude Code docs at `code.claude.com/docs/en/*`.
Codex docs redirect from `developers.openai.com/codex/*` to
`learn.chatgpt.com/docs/*`. ChatGPT "Apps/Connectors" are now "Plugins"
(`chatgpt.com/plugins`); OpenAI platform docs moved to
`developers.openai.com/api/docs`. `help.openai.com` refuses automated fetches.

**MCP spec:** newest revision 2026-07-28. Authorization servers and clients
SHOULD support Client ID Metadata Documents (CIMD); Dynamic Client
Registration (DCR) is MAY, "deprecated and retained for backwards
compatibility". <https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization>

## Server requirements the clients impose

1. Unauthenticated → **401** (Claude ignores `WWW-Authenticate` on a 200) with
   `WWW-Authenticate: Bearer resource_metadata="…", scope="…"`. Without the
   pointer Claude probes `/.well-known/oauth-protected-resource/<path>` then
   `/.well-known/oauth-protected-resource`.
2. Protected-resource metadata: `resource` must equal the URL exactly as the
   user entered it, path included. Claude uses only the first
   `authorization_servers` entry.
3. Authorization-server metadata: `code_challenge_methods_supported:["S256"]`
   (ChatGPT: required or unsupported); `registration_endpoint` (DCR);
   `client_id_metadata_document_supported:true` **and** `"none"` in
   `token_endpoint_auth_methods_supported` (Claude only picks CIMD with both);
   `authorization_response_iss_parameter_supported:true` with `iss` on every
   authorization response, errors included (RFC 9207 — ChatGPT's stable
   redirect needs it; Claude Code's v2 runtime rejects a mismatched `iss`).
4. Accept the RFC 8707 `resource` parameter; ChatGPT's docs show it as the
   bare origin (`resource=https%3A%2F%2Flitegen.example.com`).
5. Redirect URIs to accept: `https://claude.ai/api/mcp/auth_callback`
   (claude.ai, Desktop, mobile; `https://claude.com/api/mcp/auth_callback`
   appears in old snippets, unverified); loopback `http://localhost/callback`
   and `http://127.0.0.1/callback` **with any port** (Claude Code, Codex);
   `https://chatgpt.com/connector_platform_oauth_redirect` (with RFC 9207) or
   `https://chatgpt.com/connector/oauth/{callback_id}`.
6. Token endpoint: `application/x-www-form-urlencoded`; `invalid_grant` for a
   dead refresh token; rotate refresh tokens for public clients; answer
   discovery/registration/token within 10 s (refresh within 30 s). No
   `client_credentials` (neither Claude nor ChatGPT uses it).
7. Known CIMD client ids: Claude Code
   `https://claude.ai/oauth/claude-code-client-metadata` (loopback
   redirects); ChatGPT `https://chatgpt.com/oauth/client.json` (or
   `https://chatgpt.com/oauth/{callback_id}/client.json`), token auth `none`
   or `private_key_jwt`.
8. Every tool should declare `readOnlyHint` **and** `destructiveHint` (Claude
   connector directory requirement). ChatGPT treats a tool without
   `readOnlyHint: true` as a write action needing confirmation.
9. Keep the first 512 characters of server `instructions` self-contained
   (ChatGPT, Codex). Claude Code cuts instructions and upfront tool
   descriptions at 4,096 characters.
10. Network: claude.ai/Desktop connectors and ChatGPT connect from their
    clouds (Anthropic egress `160.79.104.0/21`; ChatGPT ranges at
    <https://openai.com/chatgpt-connectors.json>). A localhost or VPN-only
    server cannot be reached from them. Do not buffer streaming responses.
11. Design for the shortest limits: claude.ai/Desktop 240 s per call and
    ~150,000 characters per result; Claude Code 60 s per HTTP request unless
    raised, 25,000-token output cap; OpenAI Agents SDK 5 s default session
    timeout. Long jobs: return an id, poll, send progress notifications
    (Claude Code resets its 5-minute idle timer on them).

## Claude Code

```bash
# Key stored in ~/.claude.json (shell expands at add time)
claude mcp add --transport http --scope user litegen https://litegen.example.com/mcp \
  --header "Authorization: Bearer $LITEGEN_API_KEY"
# Key kept out of the file: project scope writes .mcp.json; single quotes keep ${…} literal
claude mcp add --transport http --scope project litegen https://litegen.example.com/mcp \
  --header 'Authorization: Bearer ${LITEGEN_API_KEY}'
claude mcp add-json litegen '{"type":"http","url":"https://litegen.example.com/mcp","headers":{"Authorization":"Bearer ${LITEGEN_API_KEY}"}}' --scope user
# OAuth: add without a header, then /mcp → litegen → Authenticate, or:
claude mcp add --transport http litegen https://litegen.example.com/mcp
claude mcp login litegen        # claude mcp logout litegen
```

`.mcp.json`: `{"mcpServers":{"litegen":{"type":"http","url":"…","headers":{"Authorization":"Bearer ${LITEGEN_API_KEY}"},"timeout":600000}}}`.
Optional per-entry `oauth` block (`clientId`, `callbackPort`,
`authServerMetadataUrl`, `scopes`), `headersHelper`, `alwaysLoad`.
Allow rule: `{"permissions":{"allow":["mcp__litegen__*"]}}`.

- Scopes: local (default; `~/.claude.json` under the project path), project
  (`.mcp.json`, shared, needs interactive approval), user (`~/.claude.json`,
  all projects). Precedence: managed > local > project > user > plugin >
  claude.ai connectors; no field merge.
- `${VAR}` / `${VAR:-default}` expand in `url` and `headers`. An unset var
  only warns and the literal `${VAR}` is sent.
- Transport `http` (alias `streamable-http`); a JSON entry with `url` but no
  `type` is read as stdio and skipped.
- Limits: output warning at 10k tokens, cap 25k (`MAX_MCP_OUTPUT_TOKENS`);
  text over 50,000 characters goes to a file; a tool may set
  `_meta["anthropic/maxResultSizeChars"]` (≤ 500,000); images count against
  the token cap and display inline (PNG/JPEG/GIF/WebP); response bodies over
  16 MiB fail.
- Tool search defers tools by default on current models; no per-server cap.
  Full tool name `mcp__<server>__<tool>` over 128 characters is dropped.
  Top-level property names must be 1–64 chars of `[A-Za-z0-9_.-]`, and the
  schema must be valid JSON Schema 2020-12, or the tool is excluded. Root
  `anyOf`/`oneOf`/`allOf` is flattened.
- `readOnlyHint`/`idempotentHint` allow retry after a dropped connection;
  `readOnlyHint` allows parallel calls.
- Timeouts: `MCP_TIMEOUT` startup 30 s; each HTTP request 60 s unless
  `MCP_TOOL_TIMEOUT` or per-server `timeout` > 60000; idle 5 min (reset by
  progress); calls over 2 min move to a background task.
- OAuth: RFC 9728 then RFC 8414 discovery; CIMD automatically, else DCR, or
  `--client-id/--client-secret/--callback-port`. Checks `iss` (RFC 9207).
- Sources: <https://code.claude.com/docs/en/mcp>,
  <https://code.claude.com/docs/en/env-vars>,
  <https://code.claude.com/docs/en/permissions>,
  <https://claude.com/docs/connectors/building/authentication.md>

## Claude Desktop and claude.ai (custom connectors)

- Add: **Customize → Connectors → Add custom connector** (individual plans;
  Free allows one custom connector). Team/Enterprise: an Owner adds it in
  **Organization settings → Connectors** (`https://claude.ai/admin-settings/connectors`)
  → Add → Custom → Web; members then Connect it from Customize → Connectors.
- Authentication choice: Sign in now / Sign in when needed / No sign-in.
  OAuth client: Claude's published identity (CIMD, recommended) / Register
  automatically (DCR) / your own client id (+ optional secret). Cannot be
  edited later: remove and re-add.
- **Request headers** (beta, limited orgs): up to 4; `authorization`,
  `x-api-key`, `x-auth-token` allowed; value sent verbatim (enter
  `Bearer <key>`); a shared org credential. With "No sign-in" this is the
  API-key path; most users will use OAuth.
- Transport: Streamable HTTP (and legacy SSE). Tools, prompts and resources
  supported; no sampling or subscriptions.
- Limits: ~150,000 characters per tool result, 240 s per call; text and
  image results.
- `claude_desktop_config.json` documents stdio servers only; whether it
  accepts `url` entries is **unverified** — document Connectors for remote
  servers. MCPB bundles are for local stdio servers; not needed here.
- Sources: <https://claude.com/docs/connectors/custom/add-unlisted.md>,
  <https://claude.com/docs/connectors/building/authentication.md>,
  <https://claude.com/docs/connectors/building/mcp.md>,
  <https://support.claude.com/en/articles/11175166-getting-started-with-custom-connectors-using-remote-mcp>

## Claude API MCP connector and Claude Agent SDK

```python
resp = client.beta.messages.create(
    model="claude-opus-5-5", max_tokens=1000, betas=["mcp-client-2025-11-20"],
    messages=[{"role": "user", "content": "…"}],
    mcp_servers=[{"type": "url", "url": "https://litegen.example.com/mcp",
                  "name": "litegen", "authorization_token": os.environ["LITEGEN_API_KEY"]}],
    tools=[{"type": "mcp_toolset", "mcp_server_name": "litegen"}],
)
```

- Beta `mcp-client-2025-11-20` (everywhere); `mcp-client-2026-09-15` adds
  tool-list pinning, Claude API only. Server fields: `type:"url"`, `url`
  (https only), `name`, `authorization_token` (sent as Bearer). No custom
  headers. `mcp_servers` max 20. Toolset allowlist:
  `default_config:{enabled:false}` + `configs:{"<tool>":{enabled:true}}`.
  Tools only. Tool names `^[a-zA-Z0-9_-]{1,128}$`.
- Agent SDK (TS `@anthropic-ai/claude-agent-sdk`, Python `claude_agent_sdk`):
  `mcpServers: { litegen: { type: "http", url, headers: { Authorization: "Bearer …" } } }`,
  `allowedTools: ["mcp__litegen__*"]` (without it tools are not
  auto-approved). No interactive OAuth (status `needs-auth`). Same limits as
  Claude Code.
- Sources: <https://platform.claude.com/docs/en/agents-and-tools/mcp-connector>,
  <https://code.claude.com/docs/en/agent-sdk/mcp>

## ChatGPT

- **chatgpt.com/plugins → + → Add custom MCP server**: name, Server URL
  (with `/mcp`), Authentication **OAuth** / No authentication / mixed,
  accept the warning, **Create as a plugin**, install it, then `@` it in a
  chat. Refresh on the details page picks up new tools. Developer mode is
  no longer required (OpenAI staff, 2026-10-01; one report says Free still
  needs the toggle — unresolved). Workspace RBAC setting is "Create custom
  MCP servers". Plan matrix unverified.
- **No API keys or custom headers**: "nor can it present custom API keys".
  LiteGen on ChatGPT is OAuth only.
- CIMD preferred (`client_id_metadata_document_supported:true`), DCR
  supported, static client credentials if provided. PKCE S256 required.
  Redirect `https://chatgpt.com/connector_platform_oauth_redirect` when the
  AS supports RFC 9207 `iss`.
- Writes allowed, confirmed per conversation; `readOnlyHint` respected.
  `search`/`fetch` needed only for deep research / company knowledge.
- Transport: Streamable HTTP (and SSE).
- Sources: <https://developers.openai.com/api/docs/guides/custom-mcp-server>,
  <https://developers.openai.com/plugins/build/auth.md>,
  <https://developers.openai.com/plugins/deploy/connect-chatgpt.md>,
  <https://developers.openai.com/plugins/plan/tools.md>

## Codex (CLI, IDE extension, ChatGPT desktop; `~/.codex/config.toml`)

```toml
[mcp_servers.litegen]
url = "https://litegen.example.com/mcp"
bearer_token_env_var = "LITEGEN_API_KEY"
startup_timeout_sec = 20
tool_timeout_sec = 600
# enabled_tools = [...]; disabled_tools = [...]; default_tools_approval_mode = "prompt"
```

```bash
codex mcp add litegen --url https://litegen.example.com/mcp --bearer-token-env-var LITEGEN_API_KEY
codex mcp add litegen --url https://litegen.example.com/mcp && codex mcp login litegen   # OAuth
codex mcp list        # TUI: /mcp
```

- Project config `.codex/config.toml` loads only in trusted projects.
  `codex mcp add` never writes `http_headers`/`env_http_headers`.
- OAuth via `codex mcp login` (CIMD if advertised, else DCR); loopback
  `http://127.0.0.1/callback`; mismatched `iss` rejected.
- Streamable HTTP and stdio (no SSE). Tool timeout: docs say 60 s, source
  300 s — set `tool_timeout_sec`. Tool names ≤ 128 bytes; `tools/list`
  capped at 2,048 items.
- Sources: <https://learn.chatgpt.com/docs/extend/mcp.md>,
  <https://learn.chatgpt.com/docs/config-file/config-reference.md>,
  github.com/openai/codex `codex-rs/cli/src/mcp_cmd.rs`

## OpenAI Responses API and Agents SDK

```json
{"type":"mcp","server_label":"litegen","server_url":"https://litegen.example.com/mcp",
 "headers":{"Authorization":"Bearer <LITEGEN_API_KEY>"},"require_approval":"always",
 "allowed_tools":["generate_image"]}
```

- Also `authorization` (OAuth token, never stored), `defer_loading`,
  `tunnel_id` for private servers. Approvals on by default
  (`mcp_approval_request` → `mcp_approval_response`).
- Python `agents.mcp.MCPServerStreamableHttp(name=…, params={"url":…, "headers":{…},
  "timeout":30, "sse_read_timeout":600}, client_session_timeout_seconds=300)` —
  the default session timeout is **5 s**. Hosted: `HostedMCPTool(tool_config={…})`.
- TS `new MCPServerStreamableHttp({ url, name, requestInit: { headers: {…} },
  timeout: 300_000, clientSessionTimeoutSeconds: 300 })`; `hostedMcpTool({…})`.
- Sources: <https://developers.openai.com/api/docs/guides/tools-connectors-mcp>,
  <https://openai.github.io/openai-agents-python/mcp/>,
  github.com/openai/openai-agents-js
