# Other clients

Cline, Continue, Goose, GitHub Copilot (the cloud agent and the CLI), Kiro,
and clients that can only start local servers.

Every section uses the hosted endpoints. For a self-hosted LiteGen, use
`{public_base_url}/mcp` and `{public_base_url}/admin/mcp` instead, for
example `https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp`
for a server on your machine.

Every client needs a LiteGen API key from the dashboard's **Keys** page: the
**AI** preset for `/mcp`, and a second key with the **Management** preset for
the management tools at `/admin/mcp`. See [permissions](../permissions.md).
To check any of them, ask: **"List LiteGen's image models."**

Video and 3D generations return a job at once, and `wait_for_generation`
waits at most 45 seconds by default (`timeout_seconds`, up to 230), so only
slow image models need a longer tool timeout. Where a client has a timeout
setting, the section sets it to ten minutes.

## Cline

### Connect with an API key

In the Cline panel, click **MCP Servers**, open the **Configure** tab and click
**Configure MCP Servers**. Add LiteGen under `mcpServers`:

```json
{
  "mcpServers": {
    "litegen": {
      "type": "streamableHttp",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" },
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

Keep `"type": "streamableHttp"`: without it Cline uses the old SSE
transport, which LiteGen does not serve. Cline's documentation shows no
environment-variable references, so the key goes in the file.

The Cline CLI reads `~/.cline/data/settings/cline_mcp_settings.json`, or adds
the server for you:

```bash
cline mcp add litegen https://app.litegen.ai/api/mcp --transport http \
  --header "Authorization: Bearer $LITEGEN_API_KEY" --yes
```

### Admin endpoint, toolsets and long jobs

- Add `litegen-admin` the same way, with `/admin/mcp` and a Management key.
- Ask LiteGen for [toolsets](../README.md#toolsets) in the URL. Tools named in
  `autoApprove` run without asking; keep that to tools that spend nothing,
  such as `list_models` and `get_model`.
- Set the server's request timeout in the MCP Servers panel.

### OAuth

Coming with OAuth support.

## Continue

MCP tools work in Continue's agent mode. Create
`.continue/mcpServers/litegen.yaml` in your workspace:

```yaml
name: LiteGen
version: 0.0.1
schema: v1
mcpServers:
  - name: LiteGen
    type: streamable-http
    url: https://app.litegen.ai/api/mcp
    requestOptions:
      headers:
        Authorization: Bearer ${{ secrets.LITEGEN_API_KEY }}
```

and put the key in `~/.continue/.env` (or the workspace's `.continue/.env`):

```text
LITEGEN_API_KEY=sk_live_…
```

Continue looks up `${{ secrets.… }}` in the workspace's `.env`, then
`.continue/.env`, then `~/.continue/.env`. The IDE extensions cannot read
variables exported in your shell; only the Continue CLI reads the process
environment.

- **Admin endpoint**: a second entry under `mcpServers`, with `/admin/mcp`
  and `${{ secrets.LITEGEN_ADMIN_KEY }}`.
- **Toolsets**: ask LiteGen for [toolsets](../README.md#toolsets) in the URL.
- **Long jobs**: `connectionTimeout` bounds only the first connection;
  Continue documents no tool-call timeout.
- **"MCP Server has unresolved secrets"**: the secret is in none of the
  `.env` files above.

## Goose

Add an extension to `~/.config/goose/config.yaml`:

```yaml
extensions:
  litegen:
    type: streamable_http
    name: litegen
    enabled: true
    uri: https://app.litegen.ai/api/mcp
    headers:
      Authorization: Bearer ${LITEGEN_API_KEY}
    env_keys: [LITEGEN_API_KEY]
    envs: {}
    timeout: 600
```

Remote extensions take `uri`, not `url`. Goose fills `${LITEGEN_API_KEY}` in
the header from the names in `env_keys`, reading each from its environment
first and then from its secret store. Goose Desktop may not see variables
exported in your shell; if the extension connects from the CLI but not from
Desktop, that is why. `goose configure` → **Add Extension** → **Remote
Extension (Streamable HTTP)** walks through the same fields.

- **Admin endpoint**: a second extension, `litegen-admin`, with `/admin/mcp`
  and `LITEGEN_ADMIN_KEY`.
- **Toolsets**: ask LiteGen for [toolsets](../README.md#toolsets) in the
  `uri`, or list the tools to load in `available_tools`.
- **Long jobs**: `timeout` is in seconds; `600` is ten minutes.

### OAuth

Coming with OAuth support.

## GitHub Copilot cloud agent

The cloud agent (formerly the coding agent) runs on GitHub, so it can only
reach a LiteGen it can get to over the internet.

1. In the repository, open **Settings → Copilot → MCP servers** and add to
   the MCP configuration:

   ```json
   {
     "mcpServers": {
       "litegen": {
         "type": "http",
         "url": "https://app.litegen.ai/api/mcp",
         "headers": { "Authorization": "Bearer $COPILOT_MCP_LITEGEN_API_KEY" },
         "tools": ["list_models", "get_model", "estimate_image_cost", "generate_image"]
       }
     }
   }
   ```

2. Add the key as an Agents secret named `COPILOT_MCP_LITEGEN_API_KEY`. The
   configuration can only reference secrets and variables whose names start
   with `COPILOT_MCP_`.

The agent runs the tools listed in `tools` without asking, so list only the
ones it needs (`["*"]` allows all), and give its key a budget. The cloud
agent does not support OAuth for remote servers. The admin endpoint is a
second entry with a Management key; most repositories should not give an
autonomous agent one.

## GitHub Copilot CLI

```bash
copilot mcp add --transport http --timeout 600000 \
  --header 'Authorization: Bearer ${LITEGEN_API_KEY}' \
  litegen https://app.litegen.ai/api/mcp
```

The single quotes keep `${LITEGEN_API_KEY}` as written, so the key stays out
of `~/.copilot/mcp-config.json`, which then holds:

```json
{
  "mcpServers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${LITEGEN_API_KEY}" },
      "tools": ["*"],
      "timeout": 600000
    }
  }
}
```

Project servers go in `.mcp.json` or `.github/mcp.json`; the CLI does not read
`.vscode/mcp.json`. `copilot mcp list` shows the configured servers, and `/mcp
show litegen` in a session shows the server's status and tools. Add the
admin endpoint as `litegen-admin`. Ask LiteGen for
[toolsets](../README.md#toolsets) in the URL, or narrow `tools`.

### OAuth

Coming with OAuth support.

## Kiro

Add LiteGen to `~/.kiro/settings/mcp.json` (every workspace) or a workspace's
`.kiro/settings/mcp.json`:

```json
{
  "mcpServers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${LITEGEN_API_KEY}" },
      "autoApprove": ["list_models", "get_model"]
    }
  }
}
```

Kiro expands only the environment variables you approve: when you save, it
warns about `LITEGEN_API_KEY`; add it to the **Mcp Approved Env Vars**
setting. Hide tools with `disabledTools`, or ask LiteGen for
[toolsets](../README.md#toolsets) in the URL. Add the admin endpoint as
`litegen-admin`.

### OAuth

Coming with OAuth support.

## Clients that only start local servers

A client that can only launch a local command can reach LiteGen through
[`mcp-remote`](https://github.com/geelen/mcp-remote), a local bridge that
needs Node.js:

```json
{
  "mcpServers": {
    "litegen": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "https://app.litegen.ai/api/mcp", "--header", "Authorization:${LITEGEN_AUTH}"],
      "env": { "LITEGEN_AUTH": "Bearer sk_live_…" }
    }
  }
}
```

Write the header as `Authorization:${…}` with no space: some clients split
arguments at spaces. `--header-file <path>` keeps the key out of the process
list.

## Troubleshooting

- **401**: the key is wrong, revoked or blocked, or the variable the config
  references was empty where the client runs.
- **The client tries SSE, or GET returns 405**: LiteGen serves Streamable
  HTTP only. Pick the client's Streamable HTTP or "http" transport.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

On 2026-11-02:
<https://docs.cline.bot/mcp/mcp-overview.md>;
<https://docs.continue.dev/customize/deep-dives/mcp> with Continue's
`docs/reference.mdx` and `docs/faqs.mdx`;
Goose's `documentation/docs/guides/config-files.md` and
`documentation/docs/getting-started/using-extensions.md` (github.com/aaif-goose/goose),
whose `resolve()` in `crates/goose/src/agents/extension.rs` fills header
variables;
<https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/extend-coding-agent-with-mcp>;
<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers>;
<https://kiro.dev/docs/mcp/configuration/>;
<https://github.com/geelen/mcp-remote>.

GitHub Copilot CLI 1.0.105 wrote the `mcp-config.json` entry above; `copilot
mcp list` and `copilot mcp get` read it back. They don't connect, and a
session needs a GitHub sign-in, so the connection itself was not checked.
The other clients were checked against docs only.
