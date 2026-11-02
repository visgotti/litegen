# JetBrains: Junie and AI Assistant

Connect JetBrains IDEs to LiteGen's MCP server, through Junie (the coding
agent, in the IDE and as a CLI) or AI Assistant.

## Before you start

- A JetBrains IDE with Junie or AI Assistant, or the Junie CLI.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).

## Junie

### Connect with an API key

Junie in the IDE and the Junie CLI share one configuration. Add LiteGen to
`~/.junie/mcp/mcp.json` (your user scope; it stays out of every project):

```json
{
  "mcpServers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" },
      "timeout": 600000
    }
  }
}
```

Junie's documentation shows no environment-variable references in
`headers`, so the key goes in the file. A project's `.junie/mcp/mcp.json`
can be committed and shared, so never put a key there.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

### Add the admin endpoint

Add a `litegen-admin` entry beside `litegen`, with
`"url": "https://app.litegen.ai/api/admin/mcp"` and a key with the
**Management** preset.

### Long jobs

Junie waits 60 seconds for any MCP request by default, and some image models
take longer. `timeout` (milliseconds, between 1,000 and 3,600,000) raises it
for one server; `600000` is ten minutes. Note `"timeout": 600` is rejected,
not read as seconds.

### Check it works

Run `/mcp` in Junie: `litegen` should be **Active**. Then ask: **"List
LiteGen's image models."**

### OAuth

Coming with OAuth support. Junie signs in to OAuth servers from `/mcp` →
the server → **Authorize**.

## AI Assistant

### Connect with an API key

Open **Settings | Tools | AI Assistant | Model Context Protocol (MCP)**, click
**Add**, choose the JSON configuration, and paste:

```json
{
  "mcpServers": {
    "litegen": {
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer sk_live_…" }
    }
  }
}
```

Choose **Global** or **Project** level, click **OK**, then **Apply**; the
Status column shows whether it connected.

AI Assistant's documentation shows only `url` for remote servers. If the
Status column shows an authentication error with a key you know works, your
version is not sending the header: use the
[`mcp-remote`](https://github.com/geelen/mcp-remote) bridge, which runs
locally and adds it.

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

`mcp-remote` needs Node.js. Writing the header as `Authorization:${…}`, with
no space, avoids clients that split arguments at spaces.

### Add the admin endpoint

A second entry, `litegen-admin`, with `/admin/mcp` and a Management key.

## Pick toolsets

Ask LiteGen for [toolsets](../README.md#toolsets) in the URL:

```text
https://app.litegen.ai/api/mcp?toolsets=images,catalog,generations
```

## Troubleshooting

- **Junie shows `Failed` or `Authorization required`**: `Failed` with a 401
  means the key is wrong, revoked or blocked. `Authorization required` means
  the entry has no `Authorization` header, so Junie expects OAuth.
- **AI Assistant shows an authentication error with a working key**: see
  the `mcp-remote` note above.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html> and
<https://www.jetbrains.com/help/ai-assistant/mcp.html> on 2026-11-02.
Checked against docs: neither was run.
