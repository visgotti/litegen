# Cursor

Connect Cursor's agent to LiteGen's MCP server.

## Before you start

- Cursor installed.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).
- The key in an environment variable Cursor can see. Set it in your shell
  profile (`export LITEGEN_API_KEY=sk_live_…` in `~/.zshrc` or `~/.bashrc`),
  or system-wide.

## Connect with an API key

Add LiteGen to `~/.cursor/mcp.json` (every project) or `.cursor/mcp.json`
(one project):

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

Cursor replaces `${env:LITEGEN_API_KEY}` when it connects, so the file holds
no secret and a project's `.cursor/mcp.json` can be committed.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

```json
{
  "mcpServers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${env:LITEGEN_API_KEY}" }
    },
    "litegen-admin": {
      "url": "https://app.litegen.ai/api/admin/mcp",
      "headers": { "Authorization": "Bearer ${env:LITEGEN_ADMIN_KEY}" }
    }
  }
}
```

`LITEGEN_ADMIN_KEY` holds a key with the **Management** preset.

## Pick toolsets

`/admin/mcp` can list up to 93 tools. Ask LiteGen for just the
[toolsets](../README.md#toolsets) you use, and switch off single tools from
**Customize** in the sidebar:

```json
"url": "https://app.litegen.ai/api/admin/mcp?toolsets=keys,budgets,spend"
```

## Long jobs

Cursor documents no tool-call timeout setting for MCP servers. Video and 3D
generations return a job at once, and `wait_for_generation` waits at most 45
seconds by default (`timeout_seconds`, up to 230) before returning the
progress so far, so long jobs never hold one call open for minutes.

## Check it works

Open **Customize** in the sidebar: `litegen` should be listed and enabled,
with its tools. In the agent chat, ask: **"List LiteGen's image models."**
Cursor asks to run `list_models`; approve it. Generated images come back
with small previews, which Cursor attaches to the chat.

## OAuth

Coming with OAuth support.

## Troubleshooting

- **401, or the server shows an error**: open the Output panel
  (Cmd/Ctrl+Shift+U) and choose **MCP Logs**. A 401 usually means
  `LITEGEN_API_KEY` was empty in Cursor's environment. Quit Cursor and start
  it from a terminal where the variable is set (`cursor .`), or set the
  variable system-wide. Otherwise the key is wrong, revoked or blocked.
- **The admin tools are missing**: the Management key lacks `admin`, or
  `?toolsets=` leaves them out.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://cursor.com/docs/mcp.md> on 2026-11-02. Checked against docs: Cursor
was not run.
