# Claude Code

Connect Claude Code (the CLI, and the IDE extensions built on it) to
LiteGen's MCP server.

## Before you start

- Claude Code installed.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).
- The key in your environment:

  ```bash
  export LITEGEN_API_KEY=sk_live_…
  ```

## Connect with an API key

Add LiteGen as a JSON entry that references the variable. Claude Code
stores `${LITEGEN_API_KEY}` as written and expands it each time it connects,
so the key stays out of its config file:

```bash
claude mcp add-json --scope user litegen \
  '{"type":"http","url":"https://app.litegen.ai/api/mcp","headers":{"Authorization":"Bearer ${LITEGEN_API_KEY}"},"timeout":600000}'
```

The single quotes stop your shell from expanding the variable. `--scope user`
makes LiteGen available in every project.

Or let the shell put the key itself into `~/.claude.json`:

```bash
claude mcp add --transport http --scope user litegen https://app.litegen.ai/api/mcp \
  --header "Authorization: Bearer $LITEGEN_API_KEY"
```

To share the server with a team, put it in the project's `.mcp.json`. Each
person sets their own `LITEGEN_API_KEY`, and Claude Code asks each of them
to approve the server the first time they run `claude` in the project:

```json
{
  "mcpServers": {
    "litegen": {
      "type": "http",
      "url": "https://app.litegen.ai/api/mcp",
      "headers": { "Authorization": "Bearer ${LITEGEN_API_KEY}" },
      "timeout": 600000
    }
  }
}
```

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

Keys, budgets, spend, logs and gateway configuration are on a second
endpoint. Add it as its own server with a Management key:

```bash
export LITEGEN_ADMIN_KEY=sk_live_…   # Management preset
claude mcp add-json --scope user litegen-admin \
  '{"type":"http","url":"https://app.litegen.ai/api/admin/mcp","headers":{"Authorization":"Bearer ${LITEGEN_ADMIN_KEY}"}}'
```

Its tools are named `mcp__litegen-admin__…`.

## Pick toolsets

Claude Code loads MCP tools on demand, so it has no tool cap and both
endpoints work as they are. To keep a session focused anyway, ask for
[toolsets](../README.md#toolsets) in the URL:

```text
https://app.litegen.ai/api/mcp?toolsets=images,catalog,generations
```

## Long jobs

Claude Code gives each MCP HTTP request 60 seconds unless told otherwise,
and some image models take longer. The `"timeout": 600000` (milliseconds) in
the entries above raises that for LiteGen; the environment variable
`MCP_TOOL_TIMEOUT=600000` raises it for every server. `claude mcp add` has no
timeout flag, so use `add-json`, or add `timeout` to the entry afterwards.

Video and 3D generations return a job at once. `wait_for_generation` waits up
to 45 seconds by default (230 at most) and returns the result, or the
progress so far; Claude calls it again until the job finishes.

## Allow tools without asking

Claude Code asks before each MCP tool call. To skip the prompt for tools that
spend nothing, add them to `permissions.allow` in `.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__litegen__list_models",
      "mcp__litegen__get_model",
      "mcp__litegen__estimate_image_cost",
      "mcp__litegen__wait_for_generation"
    ]
  }
}
```

`mcp__litegen__*` allows every tool on the endpoint, including the ones
that spend your budget.

## Check it works

```bash
claude mcp list
```

```text
litegen: https://app.litegen.ai/api/mcp (HTTP) - ✔ Connected
```

Then, in a session, ask: **"List LiteGen's image models."** Claude calls
`list_models` with `media_type: image`. `/mcp` in a session shows the
server and its tools; `claude mcp get litegen` shows the stored entry.

## OAuth

Instead of a key, Claude Code can sign in with your LiteGen account. Add the
server without a header:

```bash
claude mcp add --transport http --scope user litegen https://app.litegen.ai/api/mcp
```

`claude mcp list` then shows `! Needs authentication`. Sign in with
`claude mcp login litegen` (it opens your browser; `--no-browser` prints the
URL instead, for an SSH session), or run `/mcp` in a session and choose
**Authenticate**. On LiteGen's consent page, pick the organization, the app
and the scopes, and approve. LiteGen shows Claude Code as "Published by
claude.ai": Claude Code identifies itself with its client metadata document.
After that, `claude mcp list` shows `✔ Connected`, and Claude Code renews
its token on its own.

`claude mcp logout litegen` forgets Claude Code's tokens. To cut the app off
on LiteGen's side, revoke it under **Connected apps** on the dashboard's
Connect AI tools page.

## Troubleshooting

- **`✘ Failed to connect — Server rejected the configured Authorization
  header (HTTP 401)`**: the key is wrong, revoked or blocked, or the variable
  is not set in the shell that runs `claude`. When it is unset, `claude mcp
  list` also warns `Missing environment variables: LITEGEN_API_KEY` and
  Claude Code sends the literal text `${LITEGEN_API_KEY}`.
- **`Pending approval`**: a server from `.mcp.json` waits for you to run
  `claude` in the project and approve it.
- **A tool is missing**: the key lacks the scope, or the tool is on the other
  endpoint. See [troubleshooting](../troubleshooting.md#tools).
- **Large results**: Claude Code warns at 10,000 tokens of tool output and
  stops at 25,000 (`MAX_MCP_OUTPUT_TOKENS` raises it). LiteGen returns images
  as URLs plus small previews; filter `list_models` by `media_type` rather
  than listing every model.

## Checked against

<https://code.claude.com/docs/en/mcp> on 2026-11-02, and with Claude Code
2.1.316 against a local LiteGen server: `claude mcp add-json` with
`${LITEGEN_API_KEY}` and `claude mcp add --header` both showed
`✔ Connected`, on `/mcp` and `/admin/mcp`; with the variable unset, the
401 and the warning above. OAuth checked on 2026-11-03 with Claude Code
2.1.317 against a local server: the server added without a key showed
`! Needs authentication`; `claude mcp login --no-browser` signed in (LiteGen
fetched Claude Code's client metadata document from claude.ai, the loopback
redirect matched on Claude Code's own port, and Claude Code accepted the
`iss`), then `claude mcp list` showed `✔ Connected`.
