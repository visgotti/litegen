# LiteGen MCP server

LiteGen is an MCP server. Any MCP client — Claude Code, Claude Desktop and
claude.ai, ChatGPT, Codex, VS Code, Cursor, and agent frameworks — can use
LiteGen's API through it: generate images, video and 3D models, and manage
keys, budgets and the gateway.

There is nothing to install. Every LiteGen deployment serves two MCP
endpoints over Streamable HTTP, on the same paths LiteLLM uses:

| Endpoint | Hosted service | Self-hosted | What it serves |
|---|---|---|---|
| `/mcp` | `https://app.litegen.ai/api/mcp` | `{public_base_url}/mcp` | The AI API: models, generation, cost quotes, job status, generations and their files, mesh conversion |
| `/admin/mcp` | `https://app.litegen.ai/api/admin/mcp` | `{public_base_url}/admin/mcp` | The management API: API keys, budgets and limits, end users, spend, request logs, audit log, organizations, apps, members and gateway configuration |

Every tool is one LiteGen API operation, and a tool call runs as that API
request with your own credentials. So MCP can do exactly what your API key
(or your account) can do over HTTP, and nothing more. See
[permissions](permissions.md).

## Quick start

1. Create an API key on the dashboard's **Keys** page. Pick the **AI**
   preset for `/mcp`, or **Management** for `/admin/mcp`. See
   [permissions](permissions.md) for what each preset can do.
2. Put the key in an environment variable: `export LITEGEN_API_KEY=sk_live_…`
3. Add the server to your client.

**Claude Code**

```bash
claude mcp add --transport http --scope user litegen https://app.litegen.ai/api/mcp \
  --header "Authorization: Bearer $LITEGEN_API_KEY"
```

**Codex** (`~/.codex/config.toml`)

```toml
[mcp_servers.litegen]
url = "https://app.litegen.ai/api/mcp"
bearer_token_env_var = "LITEGEN_API_KEY"
tool_timeout_sec = 600
```

**Cursor** (`~/.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${env:LITEGEN_API_KEY}" }
    }
  }
}
```

Then ask: "List LiteGen's image models." Every other client is below.

## Connect a client

| Client | Page | API key | Sign-in (OAuth) |
|---|---|---|---|
| Claude Code | [claude-code.md](clients/claude-code.md) | Yes | Yes |
| Claude Desktop, claude.ai | [claude-apps.md](clients/claude-apps.md) | Limited (request-headers beta) | Yes |
| ChatGPT | [chatgpt.md](clients/chatgpt.md) | No | Yes |
| Codex (CLI, IDE extension) | [codex.md](clients/codex.md) | Yes | Yes |
| VS Code (GitHub Copilot), Visual Studio | [vscode.md](clients/vscode.md) | Yes | Yes |
| Cursor | [cursor.md](clients/cursor.md) | Yes | Yes |
| Windsurf | [windsurf.md](clients/windsurf.md) | Yes | Yes |
| Gemini CLI | [gemini-cli.md](clients/gemini-cli.md) | Yes | Yes |
| Zed | [zed.md](clients/zed.md) | Yes | Yes |
| JetBrains (Junie, AI Assistant) | [jetbrains.md](clients/jetbrains.md) | Yes | Junie |
| Cline, Continue, Goose, GitHub Copilot (cloud agent, CLI), Kiro, local-only clients | [other-clients.md](clients/other-clients.md) | Yes | Varies |
| OpenAI Responses API and Agents SDK, Anthropic Messages API and Claude Agent SDK, MCP SDKs | [apis-and-sdks.md](clients/apis-and-sdks.md) | Yes | Bring your own token |
| MCP Inspector (testing) | [mcp-inspector.md](clients/mcp-inspector.md) | Yes | Yes |

The sign-in column needs LiteGen's OAuth support. Until that ships, connect
with an API key.

## How the tools work

- **Find, quote, generate.** `list_models` (filter by `media_type`), then
  `get_model` for one model's parameters, `estimate_image_cost` /
  `estimate_video_cost` / `estimate_3d_model_cost` to quote, then
  `generate_image`, `generate_video` or `generate_3d_model`. Each endpoint's
  `initialize` (or `server/discover`) returns these steps as instructions to
  the model.
- **Long jobs.** `generate_image` returns finished images. `generate_video`
  and `generate_3d_model` return a job id at once; `wait_for_generation` waits
  up to `timeout_seconds` (default 45, at most 230) and returns the result,
  or the current progress with a note to call it again. Clients that send a
  progress token see the job's percentage while it waits.
- **Files.** Generated images come back as URLs plus up to two small JPEG
  previews (longest side 512 px; pass `preview: false` to skip them). Videos
  and meshes come back as `resource_link`s to their URLs.
  `convert_3d_model` takes a mesh as `source_url` or `data_base64` and
  returns the converted file embedded in the result (up to 8 MiB).
- **Errors.** A failed call is a tool error carrying LiteGen's own HTTP
  status, error code and message, for example
  `HTTP 402 budget_exceeded: …`, so the model can read it and change course.
- **Retries.** Pass `idempotency_key` on any `generate_*` call you might
  retry; a retry with the same key is never billed twice.

### Toolsets

`/mcp` has up to 22 tools and `/admin/mcp` up to 93; each caller sees the
ones their credential can use. Clients that cap tool counts, or that work
better with fewer tools, can ask for groups with `?toolsets=`:

```text
https://app.litegen.ai/api/mcp?toolsets=images,catalog,generations
https://app.litegen.ai/api/admin/mcp?toolsets=keys,budgets,spend
```

| Endpoint | Toolsets |
|---|---|
| `/mcp` | `images`, `videos`, `models3d`, `catalog`, `generations`, `account` |
| `/admin/mcp` | `keys`, `budgets`, `end_users`, `spend`, `logs`, `audit`, `orgs`, `members`, `apps`, `users`, `gateway`, `system`, `account` |

Each tool's toolset is in the [tool reference](tools.md). An unknown toolset
name is refused with the list of valid ones.

## Billing and limits

- Generation through MCP is billed and budgeted exactly like the HTTP API:
  the key's (or account's) limits, the org's, the app's and the end user's
  all apply.
- Every call through MCP carries the spend tag `mcp`, so
  `get_spend_report` with `group_by=tag` shows MCP's share. Add your own with
  the `tags` argument; attribute spend to your customers with `end_user`.
- A result's text stays under 100,000 characters. A longer list is cut, and
  the result's `_truncated` field says how many entries are shown and how to
  page or filter. `list_models` first shortens each model to its summary;
  `get_model` has the full schema.
- Clients time out long calls. Image generation can take a minute or more
  on some models, so raise your client's tool timeout: each client page says
  how.

## Publishing

[`server.json`](../../server.json) at the repository root describes both
endpoints for the [MCP Registry](https://modelcontextprotocol.io/registry/remote-servers),
under the name `ai.litegen/litegen`, so registry-aware clients can offer
LiteGen and ask for the key. It is **not published**. Publishing a name in
the `ai.litegen` namespace needs the owner of `litegen.ai` to prove it with a
DNS record, then `mcp-publisher publish`. Bump `version` in `server.json`
with each publish.

## More

- [Permissions](permissions.md): what each key and role can do through MCP.
- [Tool reference](tools.md): every tool, its arguments and what it needs.
- [Self-hosting](self-hosting.md): configuration and reverse proxies.
- [Troubleshooting](troubleshooting.md).
- [`scripts/mcp-smoke`](../../scripts/mcp-smoke/README.md): a smoke test for
  a deployment's endpoints.
