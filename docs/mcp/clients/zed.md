# Zed

Connect Zed's agent to LiteGen's MCP server.

## Before you start

- Zed installed.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).

## Connect with an API key

Open **Settings → AI → MCP Servers**, click **Add Server**, choose **Add
Remote Server**, and enter LiteGen's URL and header. Zed writes an entry like
this to your settings file (open it with **zed: open settings file**):

```json
{
  "context_servers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" }
    }
  }
}
```

Zed's documentation shows no environment-variable references in `headers`,
so the key goes in the file. Keep it in your user settings, never in a
project's `.zed/settings.json` that you commit.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

```json
{
  "context_servers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" }
    },
    "litegen-admin": {
      "url": "https://app.litegen.ai/api/admin/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" }
    }
  }
}
```

The second key has the **Management** preset.

## Pick toolsets

Ask LiteGen for [toolsets](../README.md#toolsets) in the URL, or make an
[agent profile](https://zed.dev/docs/ai/agent-profiles) that turns on only
some of LiteGen's tools:

```json
"url": "https://app.litegen.ai/api/admin/mcp?toolsets=keys,budgets,spend"
```

## Long jobs

Zed documents no tool-call timeout setting for MCP servers. Video and 3D
generations return a job at once, and `wait_for_generation` waits at most 45
seconds by default (`timeout_seconds`, up to 230) before returning the
progress so far, so long jobs never hold one call open for minutes.

## Approve tools

Zed asks before every tool call (`agent.tool_permissions.default` is
`"confirm"`). Per-tool rules use the key `mcp:<server>:<tool>`, for example
`mcp:litegen:list_models`. See
[tool permissions](https://zed.dev/docs/ai/tool-permissions).

## Check it works

In **Settings → AI → MCP Servers**, the dot beside `litegen` turns green and
its tooltip says "Server is active". In the Agent Panel, ask: **"List
LiteGen's image models."** Mentioning LiteGen by name helps the model pick
its tools.

## OAuth

Coming with OAuth support. Zed starts the MCP sign-in flow for a remote
server that has no `Authorization` header.

## Troubleshooting

- **The dot isn't green**: hover it for the reason. A 401 means the key is
  wrong, revoked or blocked.
- **Zed asks you to sign in**: the entry has no `Authorization` header, so
  Zed tried OAuth. Add the header.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://zed.dev/docs/ai/mcp> on 2026-11-02. Checked against docs: Zed was
not run.
