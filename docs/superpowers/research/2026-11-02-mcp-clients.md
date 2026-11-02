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

# IDE and other clients (plan 1 Task 11, 2026-11-02)

The IDE half of the research, read from each client's official page (and,
where the page is silent, its source). Nothing here was copied as an
instruction; pages were read as data.

## Re-checks

- **Claude Code** (<https://code.claude.com/docs/en/mcp.md>): unchanged.
  `claude mcp add --transport http <name> <url> --header "…"`;
  `streamable-http` is an alias of `http` in JSON; `claude mcp list` shows
  `✔ Connected`, `! Needs authentication` or `✘ Failed to connect` (with the
  HTTP status); `claude mcp get <name>`.
- **Codex** (<https://learn.chatgpt.com/docs/extend/mcp.md>): Streamable HTTP
  keys `url`, `auth` (`oauth` default, `chatgpt`), `bearer_token_env_var`,
  `http_headers`, `env_http_headers`, `http_headers_helper`;
  `startup_timeout_sec` (default **10**), `tool_timeout_sec` (default
  **60**), `enabled`, `required`, `enabled_tools`, `disabled_tools`,
  `default_tools_approval_mode` = `auto` | `prompt` | `writes` | `approve`
  (`writes` prompts only for tools not marked read-only),
  `tools.<tool>.approval_mode`, `tools.<tool>.output_token_limit`. IDE
  extension: gear → MCP servers → Add server → Streamable HTTP. The page
  shows `codex mcp add <name> --url …` only with `--oauth-client-id`;
  `--bearer-token-env-var` is from the source (above).

## VS Code (GitHub Copilot)

```json
{
  "inputs": [{ "type": "promptString", "id": "litegen-key", "description": "LiteGen API key", "password": true }],
  "servers": {
    "litegen": { "type": "http", "url": "https://litegen.example.com/mcp",
                 "headers": { "Authorization": "Bearer ${input:litegen-key}" } }
  }
}
```

- Two formats. **VS Code format**: `.vscode/mcp.json` or the profile's
  `mcp.json` (MCP: Open User Configuration); top-level `servers` and
  `inputs`; `${env:NAME}`, `${input:id}` (`promptString`, `password: true`),
  `envFile`; stdio, HTTP and SSE. **Portable Copilot format**: workspace-root
  `.mcp.json`, or `$COPILOT_HOME/mcp-config.json` (default
  `~/.copilot/mcp-config.json`); top-level `mcpServers`; `${NAME}` and
  `${NAME:-default}` in stdio config and HTTP headers. Copilot harness
  sessions read only the portable format and skip servers that need
  `${input:…}`.
- HTTP entry: `type: "http"`, `url`, `headers`, optional `oauth.clientId`
  (a confidential client's secret goes in through the Set Client Secret
  CodeLens, kept in secret storage). OAuth runs automatically on first
  connection.
- `code --add-mcp '{"name":"…","type":"http","url":"…","headers":{…}}'` writes
  the user profile. Install link: `vscode:mcp/install?` +
  `encodeURIComponent(JSON.stringify(obj))` (`vscode-insiders:` for
  Insiders), same object as `--add-mcp`.
- Commands: MCP: Add Server, MCP: List Servers (start, stop, restart, show
  output), MCP: Reset Cached Tools. Tools are switched on and off in Configure
  Tools / Chat: Open Customizations > Tools. No tool cap and no per-server
  timeout documented. Server names: camelCase recommended.
- Sources: <https://code.visualstudio.com/docs/agents/reference/mcp-configuration>,
  <https://code.visualstudio.com/docs/copilot/customization/mcp-servers>,
  <https://code.visualstudio.com/docs/copilot/guides/mcp-developer-guide>

## Visual Studio (2022 17.14+, 2026)

- Files, in discovery order: `%USERPROFILE%\.mcp.json` (all solutions),
  `<SOLUTIONDIR>\.vs\mcp.json` (per user and solution),
  `<SOLUTIONDIR>\.mcp.json` (source control), `<SOLUTIONDIR>\.vscode\mcp.json`,
  `<SOLUTIONDIR>\.cursor\mcp.json`. Top-level `servers`; a remote entry is
  `{"url": "…"}`; transports stdio, SSE and HTTP. Saving the file restarts the
  agent and reloads servers.
- OAuth for remote servers with any provider, per the MCP authorization spec
  (CodeLens: Manage Authentication). Server trust from 2026 version 18.7.
  Agent mode only.
- The page documents no `headers`, `inputs` or variable references. Visual
  Studio reads `.vscode/mcp.json`, so the VS Code format probably applies:
  **unverified**.
- Source: <https://learn.microsoft.com/en-us/visualstudio/ide/mcp-servers>
  (updated 2026-07-30)

## Cursor

```json
{ "mcpServers": { "litegen": { "url": "https://litegen.example.com/mcp",
  "headers": { "Authorization": "Bearer ${env:LITEGEN_API_KEY}" } } } }
```

- `~/.cursor/mcp.json` (global) or `.cursor/mcp.json` (project).
  Interpolation in `command`, `args`, `env`, `url`, `headers`: `${env:NAME}`,
  `${userHome}`, `${workspaceFolder}`, `${workspaceFolderBasename}`,
  `${pathSeparator}`. `envFile` is stdio-only.
- OAuth: authorization code + PKCE, RFC 9728 and RFC 8707, DCR as a public
  client; static `auth: {CLIENT_ID, CLIENT_SECRET, scopes}`; redirect URIs
  `https://www.cursor.com/agents/mcp/oauth/callback` (web, Cloud Agents) and
  `http://localhost:8787/callback` (desktop).
- Install link: `cursor://anysphere.cursor-deeplink/mcp/install?name=<name>&config=<base64>`,
  where the config is the server object without the name wrapper.
- Logs: Output panel → MCP Logs. Tool calls ask for approval by default (Run
  Modes apply). Image results are attached to the chat. No tool cap or
  timeout setting on the page.
- Sources: <https://cursor.com/docs/mcp.md>,
  <https://cursor.com/docs/mcp/install-links.md>

## Windsurf (Cascade)

```json
{ "mcpServers": { "litegen": { "serverUrl": "https://litegen.example.com/mcp",
  "headers": { "Authorization": "Bearer ${env:LITEGEN_API_KEY}" } } } }
```

- Cascade's **Open MCP config file** opens `~/.config/devin/mcp_config.json`
  (`$XDG_CONFIG_HOME/devin/…`; Windows `%AppData%/devin/mcp_config.json`).
  The editor's discovery file is `~/.codeium/windsurf/mcp_config.json`
  (`windsurf-next` for Next).
- Remote servers take `serverUrl` or `url`. Transports stdio, Streamable HTTP
  and SSE; OAuth for HTTP and SSE. `${env:VAR}` interpolation in `command`,
  `args`, `env`, `serverUrl`, `url`, `headers`. Press refresh after adding.
  Per-server `disabledTools`. A team allowlist blocks every server not on it.
  No tool cap or timeout on the page.
- Source: <https://docs.windsurf.com/plugins/cascade/mcp.md>

## Gemini CLI

```bash
gemini mcp add --transport http --scope user --header "Authorization: Bearer $LITEGEN_API_KEY" \
  litegen https://litegen.example.com/mcp
```

- `~/.gemini/settings.json` (user) or `.gemini/settings.json` (project;
  `gemini mcp add` defaults to project). Entry: `httpUrl` (Streamable HTTP;
  `url` means SSE), `headers`, `timeout` in ms (default 600,000), `trust`,
  `includeTools`, `excludeTools`. `gemini mcp list`, `/mcp`.
- `$VAR` / `${VAR}` expansion is documented for `env`; the page is silent on
  `headers`, but 0.66.0 expands both forms there (verified, below).
- Tools are named `mcp_<server>_<tool>`; names over 63 characters are
  truncated; server names should not contain `_`. `$schema` and
  `additionalProperties` are stripped from schemas.
- OAuth: discovered on a 401; `/mcp auth <server>`; tokens in
  `~/.gemini/mcp-oauth-tokens.json`; checks `iss`; callback
  `http://localhost:<random-port>/oauth/callback`.
- Source: <https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>
  (geminicli.com/docs/tools/mcp-server)

## Zed

- `settings.json` → `context_servers.<name>`: `url`, `headers`. Settings → AI
  → MCP Servers → Add Server → Add Remote Server writes it. With no
  `Authorization` header Zed runs the MCP OAuth flow.
- Tools and prompts only. Approval: `agent.tool_permissions.default`
  (`confirm`, `allow`, `deny`); per-tool keys `mcp:<server>:<tool>`. Agent
  profiles can enable a subset of a server's tools. No variable
  interpolation or timeout documented.
- Source: <https://zed.dev/docs/ai/mcp> (github.com/zed-industries/zed
  `docs/src/ai/mcp.md`)

## JetBrains: Junie and AI Assistant

- **Junie** (CLI and IDE share it): `~/.junie/mcp/mcp.json` (user) or
  `.junie/mcp/mcp.json` (project); `mcpServers.<name>` with `url` and
  `headers`. `timeout` in ms, 1,000–3,600,000; default 60 s for every
  request, tool calls included. OAuth: `/mcp` → server → Authorize. No
  variable interpolation documented.
- **AI Assistant 2026.2**: Settings | Tools | AI Assistant | Model Context
  Protocol (MCP) → Add → JSON `{"mcpServers":{"<name>":{"url":"…"}}}`
  (Streamable HTTP; SSE for legacy servers), global or project level. The
  page documents no `headers`; Import from Claude copies Claude Desktop's
  config.
- Sources: <https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html>
  (updated 2026-10-30), <https://www.jetbrains.com/help/ai-assistant/mcp.html>

## Cline

- `mcpServers.<name>`: `type: "streamableHttp"` (omitting `type` means legacy
  SSE), `url`, `headers`, `disabled`, `autoApprove`. IDE: MCP Servers →
  Configure → Configure MCP Servers, or the Remote Servers tab. CLI file
  `~/.cline/data/settings/cline_mcp_settings.json` (`CLINE_MCP_SETTINGS_PATH`);
  `cline mcp add <name> <url> --transport http --header "Authorization: Bearer …" --yes`.
  OAuth: `cline mcp` → Authorize OAuth. Request timeout set per server in MCP
  settings. No variable interpolation documented.
- Source: <https://docs.cline.bot/mcp/mcp-overview.md>

## Continue

```yaml
mcpServers:
  - name: LiteGen
    type: streamable-http
    url: https://litegen.example.com/mcp
    requestOptions:
      headers:
        Authorization: Bearer ${{ secrets.LITEGEN_API_KEY }}
```

- Agent mode only. A block file in `.continue/mcpServers/` needs `name`,
  `version`, `schema: v1`; JSON configs from other clients can be dropped in
  the same folder. `requestOptions` applies to `sse` and `streamable-http`
  servers ("same format as model requestOptions", which has `headers`); the
  source also accepts `apiKey`, sent as `Authorization: Bearer`.
  `connectionTimeout` bounds the first connection (20 s default in source).
- `${{ secrets.NAME }}` resolves from `<workspace>/.env`,
  `<workspace>/.continue/.env`, `~/.continue/.env`, then (CLI only) the
  process environment: IDE extensions cannot read shell variables. OAuth
  only for SSE servers in source.
- Sources: <https://docs.continue.dev/customize/deep-dives/mcp>,
  github.com/continuedev/continue `docs/reference.mdx`, `docs/faqs.mdx`,
  `core/context/mcp/MCPConnection.ts`

## Goose

```yaml
extensions:
  litegen:
    type: streamable_http
    name: litegen
    enabled: true
    uri: https://litegen.example.com/mcp
    headers:
      Authorization: Bearer ${LITEGEN_API_KEY}
    env_keys: [LITEGEN_API_KEY]
    envs: {}
    timeout: 600
```

- `~/.config/goose/config.yaml`; `goose configure` → Add Extension → Remote
  Extension (Streamable HTTP); `goose session --with-streamable-http-extension <url>`.
  Remote extensions use `uri`. `timeout` in seconds (examples use 300).
  `available_tools` limits the tools loaded. SSE is not supported.
- Header values expand `$VAR` / `${VAR}` from `envs` and `env_keys`; an
  `env_keys` name is read from the process environment, else goose's secret
  store (source: `resolve()` in `crates/goose/src/agents/extension.rs`).
- OAuth: CIMD or DCR automatically; `client_id`, `client_secret_key`,
  `scopes` for pre-registered clients; callback on 127.0.0.1 with an
  ephemeral port (`GOOSE_OAUTH_CALLBACK_PORT`). Deeplink
  `goose://extension?url=<encoded>&type=streamable_http&id=…&name=…&timeout=…`.
- Repository moved from block/goose to aaif-goose/goose.
- Sources: github.com/aaif-goose/goose `documentation/docs/getting-started/using-extensions.md`,
  `documentation/docs/guides/config-files.md`

## GitHub Copilot cloud agent (coding agent) and Copilot CLI

- **Cloud agent**: repository Settings → Copilot → MCP servers → MCP
  configuration (JSON, `mcpServers`). Required `tools` (allowlist, `["*"]`
  for all; the agent runs tools without asking) and `type` (`local`,
  `stdio`, `http`, `sse`); remote `url` and `headers`. Values may reference
  Agents secrets or variables named `COPILOT_MCP_…` as `$NAME`, `${NAME}` or
  `${NAME:-default}`. No OAuth for remote servers. Copilot code review uses
  only tools with `readOnlyHint: true`.
- **Copilot CLI**: `copilot mcp add --transport http --header "Authorization: Bearer …" <name> <url>`
  (`--timeout MS`); `/mcp add`; `~/.copilot/mcp-config.json` →
  `mcpServers.<name>`: `type: "http"`, `url`, `headers`, `tools: ["*"]`.
  Project files `.mcp.json` and `.github/mcp.json`; `.vscode/mcp.json` is not
  read. `copilot mcp list`, `copilot mcp get <name>`, `/mcp show <name>`.
  Header values expand `${NAME}` (VS Code's portable-format table).
- Sources: <https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/extend-coding-agent-with-mcp>,
  <https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers>

## Kiro

- `.kiro/settings/mcp.json` (workspace) or `~/.kiro/settings/mcp.json`
  (user); `mcpServers.<name>`: `url` (HTTPS, or HTTP for localhost),
  `headers`, `autoApprove`, `disabledTools`, `oauth`, `oauthScopes`.
  `${VAR}` expands only for variables approved in the **Mcp Approved Env
  Vars** setting (Kiro shows a warning listing unapproved ones). OAuth:
  browser flow, DCR, or `oauth.clientId` (public clients only in the IDE).
  No timeout documented.
- Source: <https://kiro.dev/docs/mcp/configuration/>

## Bridges and tooling

- **mcp-remote** (stdio bridge for clients that only run local servers):
  `npx mcp-remote <url> --header "Authorization: Bearer ${AUTH_TOKEN}"` with
  `AUTH_TOKEN` in `env`. Cursor, Codex CLI and Claude Desktop on Windows
  mangle spaces in `args`: use `"Authorization:${AUTH_HEADER}"` with
  `AUTH_HEADER=Bearer …`. `--header-file` keeps the key out of the process
  list. `--headers-timeout` / `--body-timeout` default 300 s.
  Source: <https://github.com/geelen/mcp-remote>
- **MCP Inspector v2** (Node ≥ 22.19): CLI flags changed from v1 —
  `npx @modelcontextprotocol/inspector --cli --transport http --server-url <url> --header 'Authorization: Bearer …' --method tools/list --format json`;
  `--method tools/call --tool-name <t> --tool-args-json '{…}'`. Exit codes:
  0 ok, 1 usage, 3 auth required, 4 unreachable, 5 tool error, 6 `--strict`
  schema problem. v1: `@modelcontextprotocol/inspector@v1-latest`.
  Sources: <https://github.com/modelcontextprotocol/inspector>,
  `docs/cli-smoke-testing.md`
- **MCP SDKs.** TypeScript `@modelcontextprotocol/sdk` 1.32.2 (npm `latest`):
  `StreamableHTTPClientTransport(url, { requestInit: { headers } })`;
  `callTool(params, resultSchema?, { timeout })`, default request timeout
  60,000 ms. Python `mcp` 2.3.0: `Client(streamable_http_client(url, http_client=httpx2.AsyncClient(headers=…, timeout=…)))`
  (a bare URL string gets a client without headers); `call_tool(name, args,
  read_timeout_seconds=…, progress_callback=…)`. OpenAI Agents Python:
  `params.timeout` (default 30) bounds connect/write and
  `params.sse_read_timeout` (default 300) bounds reads
  (`src/agents/mcp/server.py`); `client_session_timeout_seconds` default 5.
- **MCP Registry** (preview): `server.json` with `$schema`
  `https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json`,
  `name`, `title`, `description`, `version`, `remotes[]` of `type`
  (`streamable-http` | `sse`), `url`, `headers[]` (`name`, `description`,
  `isRequired`, `isSecret`). A remote must be publicly reachable. Published
  with `mcp-publisher` (`init`, `validate`, `publish`).
  Source: <https://modelcontextprotocol.io/registry/remote-servers.md>
  The 2025-12-11 schema also gives a header `value` (`"Bearer {api_key}"`)
  with `variables`, so a client prompts for the key alone; LiteGen's
  `server.json` uses that and validates against the schema with no errors.

## Checked by hand (2026-11-02)

Against a local LiteGen (single-tenant, master key set so a bad header gets a
real 401, mock provider), with each client's config isolated in a scratch
directory:

| Client | Version | Result |
|---|---|---|
| Smoke script (`@modelcontextprotocol/sdk`) | 1.32.2 | AI key on `/mcp`: 21 tools, 99 image models, `generate_image` on `mock/visual-image-gen` with a preview; Management key on `/admin/mcp`: 62 tools; no key: 401 |
| MCP Python SDK | 2.3.0 | The `httpx2` header pattern above: 21 tools, 99 image models |
| MCP Inspector CLI | 2.13.0 | `initialize`, `tools/list`, `tools/call` work; wrong key exits 3 (`auth_required`, it then tries interactive OAuth); `--strict` found 40 `oneOf: [{}, …]` members (an MCP schema bug, fixed) and now reports 0 errors, warnings only on free-form fields |
| Claude Code (`CLAUDE_CONFIG_DIR` scratch) | 2.1.316 | `add-json` with `${LITEGEN_API_KEY}` stored as written and `✔ Connected`; `add --header` too; unset variable: `✘ Failed to connect — Server rejected the configured Authorization header (HTTP 401)` plus `Missing environment variables: LITEGEN_API_KEY`; `claude mcp get` shows `Timeout: 600000ms` |
| Gemini CLI (scratch `HOME`) | 0.66.0 | `gemini mcp add --transport http` writes `"type": "http"` + `url`; `$VAR` and `${VAR}` expand in `headers`; MCP servers, user-level included, are disabled in an untrusted folder (`GEMINI_CLI_TRUST_WORKSPACE=true` or `--skip-trust` for scripts) |
| Codex (scratch `CODEX_HOME`) | 0.166.0 | `codex mcp add <name> --url … --bearer-token-env-var VAR` writes `url` + `bearer_token_env_var`; `codex mcp list` shows `Auth: Bearer token`; no connect check without a session |
| GitHub Copilot CLI (scratch `HOME`) | 1.0.105 | `copilot mcp add --transport http --timeout 600000 --header '…'` writes `type`, `url`, `headers`, `tools: ["*"]`, `timeout`; single-quoted `${VAR}` kept as written; `mcp list`/`mcp get` read config only |

Claude Code's real `~/.claude.json` was unchanged (same mtime and size).
