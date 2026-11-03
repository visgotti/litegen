# Windsurf

Connect Windsurf's Cascade agent to LiteGen's MCP server.

## Before you start

- Windsurf installed. On Teams and Enterprise plans an admin may need to
  turn MCP on, and a team allowlist blocks every server not on it.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).
- The key in an environment variable Windsurf can see:
  `export LITEGEN_API_KEY=sk_live_…` in your shell profile, or system-wide.

## Connect with an API key

In Cascade's MCP menu choose **Open MCP config file**, which opens
`mcp_config.json` (on macOS and Linux `~/.config/devin/mcp_config.json`, on
Windows `%AppData%/devin/mcp_config.json`), and add:

```json
{
  "mcpServers": {
    "litegen": {
      "serverUrl": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${env:LITEGEN_API_KEY}" }
    }
  }
}
```

Windsurf replaces `${env:LITEGEN_API_KEY}` when it connects. Save the file,
then press the refresh button in the MCP panel.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

```json
{
  "mcpServers": {
    "litegen": {
      "serverUrl": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${env:LITEGEN_API_KEY}" }
    },
    "litegen-admin": {
      "serverUrl": "https://app.litegen.ai/api/admin/mcp",
      "headers": { "Authorization": "Bearer ${env:LITEGEN_ADMIN_KEY}" }
    }
  }
}
```

`LITEGEN_ADMIN_KEY` holds a key with the **Management** preset.

## Pick toolsets

Ask LiteGen for [toolsets](../README.md#toolsets) in the URL, or list tools
to hide in the server's `disabledTools`:

```json
"litegen-admin": {
  "serverUrl": "https://app.litegen.ai/api/admin/mcp?toolsets=keys,budgets,spend",
  "headers": { "Authorization": "Bearer ${env:LITEGEN_ADMIN_KEY}" },
  "disabledTools": ["revoke_api_key", "rotate_api_key"]
}
```

## Long jobs

Windsurf documents no tool-call timeout setting for MCP servers. Video and
3D generations return a job at once, and `wait_for_generation` waits at most
45 seconds by default (`timeout_seconds`, up to 230) before returning the
progress so far, so long jobs never hold one call open for minutes.

## Check it works

After refreshing, `litegen` appears in Cascade's MCP panel with its tools.
Ask Cascade: **"List LiteGen's image models."** It calls `list_models` with
`media_type: image`.

## OAuth

Instead of a key, Windsurf can sign in with your LiteGen account. Leave out
`headers`:

```json
{
  "mcpServers": {
    "litegen": {
      "serverUrl": "https://app.litegen.ai/api/mcp"
    }
  }
}
```

After you refresh, Windsurf finds LiteGen's sign-in from the server's
answer and opens LiteGen's sign-in page in your browser: pick the
organization, the app and the scopes, and approve. To cut Windsurf off on
LiteGen's side, revoke it under **Connected apps** on the dashboard's
Connect AI tools page.

## Troubleshooting

- **401**: the key is wrong, revoked or blocked, or `LITEGEN_API_KEY` was
  empty when Windsurf started. Start Windsurf from a terminal where the
  variable is set.
- **LiteGen doesn't appear**: press refresh in the MCP panel, and check you
  edited the file **Open MCP config file** opens; the editor's discovery
  file (`~/.codeium/windsurf/mcp_config.json`) is a different one.
- **Your team blocks it**: once a team allowlists any MCP server, every
  other server is blocked. Ask your admin to allowlist LiteGen.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://docs.windsurf.com/plugins/cascade/mcp.md> on 2026-11-02. Checked
against docs: Windsurf was not run.
