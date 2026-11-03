# Gemini CLI

Connect Google's Gemini CLI to LiteGen's MCP server.

## Before you start

- Gemini CLI installed (`npm install -g @google/gemini-cli`, or run it with
  `npx @google/gemini-cli`).
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).
- The key in your environment:

  ```bash
  export LITEGEN_API_KEY=sk_live_…
  ```

## Connect with an API key

Add LiteGen to `~/.gemini/settings.json` (every project) or a project's
`.gemini/settings.json`:

```json
{
  "mcpServers": {
    "litegen": {
      "httpUrl": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer $LITEGEN_API_KEY" },
      "timeout": 600000
    }
  }
}
```

Gemini CLI replaces `$LITEGEN_API_KEY` (or `${LITEGEN_API_KEY}`) when it
connects, so the file holds no secret. `httpUrl` means Streamable HTTP; a
plain `url` means the older SSE transport unless the entry also says
`"type": "http"`.

Or add the server from the command line. The shell expands the variable, so
this writes the key itself into the settings file:

```bash
gemini mcp add --scope user --transport http \
  --header "Authorization: Bearer $LITEGEN_API_KEY" \
  litegen https://app.litegen.ai/api/mcp
```

Without `--scope user`, `gemini mcp add` writes the current project's
`.gemini/settings.json`.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

```json
{
  "mcpServers": {
    "litegen": {
      "httpUrl": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer $LITEGEN_API_KEY" },
      "timeout": 600000
    },
    "litegen-admin": {
      "httpUrl": "https://app.litegen.ai/api/admin/mcp",
      "headers": { "Authorization": "Bearer $LITEGEN_ADMIN_KEY" }
    }
  }
}
```

`LITEGEN_ADMIN_KEY` holds a key with the **Management** preset. Keep
underscores out of server names: Gemini names tools
`mcp_<server>_<tool>` and splits that name at the first underscore after
`mcp_`.

## Pick toolsets

Ask LiteGen for [toolsets](../README.md#toolsets) in the URL, or filter on
Gemini's side with `includeTools` or `excludeTools`:

```json
"litegen": {
  "httpUrl": "https://app.litegen.ai/api/mcp?toolsets=images,catalog,generations",
  "headers": { "Authorization": "Bearer $LITEGEN_API_KEY" },
  "includeTools": ["list_models", "get_model", "estimate_image_cost", "generate_image"]
}
```

## Long jobs

`timeout` is in milliseconds and defaults to 600,000 (10 minutes), which
already covers slow image models. Video and 3D generations return a job at
once; `wait_for_generation` waits up to 45 seconds by default (230 at most)
and returns the result, or the progress so far.

## Check it works

```bash
gemini mcp list
```

```text
✓ litegen: https://app.litegen.ai/api/mcp (http) - Connected
```

In a session, `/mcp` lists LiteGen's tools, as `mcp_litegen_list_models` and
so on. Ask: **"List LiteGen's image models."**

## OAuth

Instead of a key, Gemini CLI can sign in with your LiteGen account. Add the
server with no header:

```bash
gemini mcp add --scope user --transport http litegen https://app.litegen.ai/api/mcp
```

In a session, run `/mcp auth litegen`. Gemini CLI opens LiteGen's sign-in
page in your browser: pick the organization, the app and the scopes, and
approve. It listens for the answer on `localhost`, checks it comes from
LiteGen (`iss`), and keeps the tokens in `~/.gemini/mcp-oauth-tokens.json`.
To cut it off on LiteGen's side, revoke it under **Connected apps** on the
dashboard's Connect AI tools page.

## Troubleshooting

- **`MCP servers are configured but disabled because this folder is
  untrusted`**: Gemini CLI connects to MCP servers, user-level ones
  included, only in a trusted folder. Trust the folder when Gemini asks, or
  with `/permissions`. For scripts and CI, set
  `GEMINI_CLI_TRUST_WORKSPACE=true` or pass `--skip-trust`.
- **`Disconnected`**: usually a 401. The key is wrong, revoked or blocked, or
  `LITEGEN_API_KEY` is not set where `gemini` runs; an unset variable
  expands to an empty string.
- **Don't set `"trust": true` on `litegen-admin`**: it skips the confirmation
  for every tool, including revoking keys and changing budgets.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>
and <https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/trusted-folders.md>
on 2026-11-02, and with Gemini CLI 0.66.0 against a local LiteGen server:
entries with `$LITEGEN_API_KEY`, with `${LITEGEN_API_KEY}` and from
`gemini mcp add --header` all showed `Connected` in a trusted folder; with
the variable unset, the first two showed `Disconnected`.

OAuth checked on 2026-11-03 with Gemini CLI 0.66.0 against a local LiteGen
server: after `gemini mcp add` with no header, `/mcp auth litegen` in a
session registered Gemini CLI, printed the sign-in URL (no browser could
open), took the answer on `localhost`, and said "Successfully authenticated
and reloaded tools"; `gemini mcp list` then showed `Connected`. Gemini CLI
asks for every scope LiteGen lists; LiteGen keeps the scopes of the
endpoint in the server entry's URL.
