# VS Code and Visual Studio

Connect GitHub Copilot's agent mode in VS Code, or in Visual Studio, to
LiteGen's MCP server.

## Before you start

- VS Code with GitHub Copilot, or Visual Studio 2022 17.14 or later
  (Visual Studio 2026 included). MCP tools work in agent mode.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).

## VS Code

### Connect with an API key

Run **MCP: Open User Configuration** from the Command Palette (or create
`.vscode/mcp.json` in a workspace) and add:

```json
{
  "inputs": [
    {
      "type": "promptString",
      "id": "litegen-api-key",
      "description": "LiteGen API key (AI preset)",
      "password": true
    }
  ],
  "servers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${input:litegen-api-key}" }
    }
  }
}
```

VS Code asks for the key the first time the server starts and keeps it in
its secret storage, so the file holds no secret and a workspace copy can be
committed.

To use an environment variable instead, write
`"Authorization": "Bearer ${env:LITEGEN_API_KEY}"`. Or add the server from a
terminal (this writes your user configuration):

```bash
code --add-mcp '{"name":"litegen","type":"http","url":"https://app.litegen.ai/api/mcp","headers":{"Authorization":"Bearer ${env:LITEGEN_API_KEY}"}}'
```

**Sessions on the Copilot harness.** VS Code runs each chat session on the
Local harness or the Copilot harness, in the Chat view or the Agents window.
Copilot-harness sessions are not given servers that need an `${input:…}`
prompt. For those, use `${env:LITEGEN_API_KEY}` in the entry above, or put
LiteGen in the portable format, in `.mcp.json` at the workspace root or in
`~/.copilot/mcp-config.json`:

```json
{
  "mcpServers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${LITEGEN_API_KEY}" }
    }
  }
}
```

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

### Add the admin endpoint

Add a second input and server; VS Code recommends camelCase server names:

```json
{
  "inputs": [
    { "type": "promptString", "id": "litegen-api-key", "description": "LiteGen API key (AI preset)", "password": true },
    { "type": "promptString", "id": "litegen-admin-key", "description": "LiteGen API key (Management preset)", "password": true }
  ],
  "servers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${input:litegen-api-key}" }
    },
    "litegenAdmin": {
      "type": "http",
      "url": "https://app.litegen.ai/api/admin/mcp",
      "headers": { "Authorization": "Bearer ${input:litegen-admin-key}" }
    }
  }
}
```

### Pick toolsets

Turn individual tools off in the chat's **Configure Tools** picker, or ask
LiteGen for [toolsets](../README.md#toolsets) so the server lists fewer:

```text
https://app.litegen.ai/api/admin/mcp?toolsets=keys,budgets,spend
```

After changing the URL or the key's scopes, run **MCP: Reset Cached Tools**.

### Long jobs

VS Code documents no tool-call timeout setting for MCP servers. Video and 3D
generations return a job at once, and `wait_for_generation` waits at most 45
seconds by default (`timeout_seconds`, up to 230) before returning the
progress so far, so long jobs never hold one call open for minutes.

### Check it works

Run **MCP: List Servers**: `litegen` should be running. In the Chat view,
switch to **Agent**, then ask: **"List LiteGen's image models."** Copilot
asks to run `list_models`; allow it.

### OAuth

Instead of a key, VS Code can sign in with your LiteGen account. Leave out
`headers`:

```json
{
  "servers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp"
    }
  }
}
```

When the server starts, VS Code finds LiteGen's sign-in from the server's
answer, registers itself and opens LiteGen's sign-in page in your browser:
pick the organization, the app and the scopes, and approve. VS Code keeps
the tokens in its secret storage and renews them. To cut it off on
LiteGen's side, revoke it under **Connected apps** on the dashboard's
Connect AI tools page.

### Troubleshooting

- **The server fails to start or shows 401**: run **MCP: List Servers**,
  select `litegen`, then **Show Output** for the server's log. A 401 means the
  key is wrong, revoked or blocked, or `${env:LITEGEN_API_KEY}` is empty
  because VS Code was started without the variable.
- **LiteGen is missing from a Copilot-harness session**: the server uses an
  `${input:…}` prompt; see above.
- **Tools are stale** after a scope or toolset change: **MCP: Reset Cached
  Tools**.
- More in [troubleshooting](../troubleshooting.md).

## Visual Studio

### Connect with an API key

Visual Studio reads, in order, `%USERPROFILE%\.mcp.json` (every solution),
`<solution>\.vs\mcp.json`, `<solution>\.mcp.json`,
`<solution>\.vscode\mcp.json` and `<solution>\.cursor\mcp.json`. Use the
first one: it is per user and stays out of source control. Edit it in Visual
Studio so the schema applies:

```json
{
  "servers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" }
    }
  }
}
```

Visual Studio's documentation does not say whether it expands `${input:…}`
or `${env:…}`, so this example holds the key itself. Never commit a file
with a key in it. Saving the file restarts the agent and reloads the
servers. **Self-hosted:** use `{public_base_url}/mcp` as the `url`.

### Add the admin endpoint

Add `litegenAdmin` beside `litegen`, with
`"url": "https://app.litegen.ai/api/admin/mcp"` and a Management key.

### Pick toolsets and check it works

In the chat panel, choose **Agent** mode and pick LiteGen's tools in the tool
list, or ask LiteGen for [toolsets](../README.md#toolsets) in the URL. Then
ask: **"List LiteGen's image models."** Copilot asks before it runs a tool;
choose **Allow**.

### OAuth

Instead of a key, Visual Studio can sign in with your LiteGen account: add
the server to `.mcp.json` with no `headers`, then sign in from the **Manage
Authentication** CodeLens above the entry. LiteGen's sign-in page opens:
pick the organization, the app and the scopes, and approve.

## Checked against

<https://code.visualstudio.com/docs/agents/reference/mcp-configuration>,
<https://code.visualstudio.com/docs/copilot/customization/mcp-servers> and
<https://learn.microsoft.com/en-us/visualstudio/ide/mcp-servers> on
2026-11-02. Checked against docs: neither editor was run.
