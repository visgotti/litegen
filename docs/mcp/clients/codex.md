# Codex

Connect OpenAI's Codex to LiteGen's MCP server. The Codex CLI, the IDE
extension and the Codex app read the same file, `~/.codex/config.toml`.

## Before you start

- Codex installed.
- A LiteGen API key from the dashboard's **Keys** page, with the **AI**
  preset for `/mcp`. The management tools at `/admin/mcp` need a second key
  with the **Management** preset. See [permissions](../permissions.md).
- The key in the environment Codex runs in:

  ```bash
  export LITEGEN_API_KEY=sk_live_…
  ```

## Connect with an API key

Add this to `~/.codex/config.toml`:

```toml
[mcp_servers.litegen]
url = "https://app.litegen.ai/api/mcp"
bearer_token_env_var = "LITEGEN_API_KEY"
tool_timeout_sec = 600
default_tools_approval_mode = "writes"
```

- `bearer_token_env_var` names the variable that holds the key. Codex reads it
  when it connects and sends it as `Authorization: Bearer …`, so the key never
  lands in the file.
- `default_tools_approval_mode = "writes"` runs read-only tools (model
  listings, cost quotes, job status) without asking, and asks before
  anything that generates or changes data. LiteGen marks which of its tools
  are read-only.

Or add the server from the command line, then add `tool_timeout_sec` to the
entry it writes:

```bash
codex mcp add litegen --url https://app.litegen.ai/api/mcp --bearer-token-env-var LITEGEN_API_KEY
```

In the IDE extension, restart the extension after editing the file; the
gear menu's **MCP servers** lists LiteGen.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

```toml
[mcp_servers.litegen-admin]
url = "https://app.litegen.ai/api/admin/mcp"
bearer_token_env_var = "LITEGEN_ADMIN_KEY"
default_tools_approval_mode = "writes"
```

`LITEGEN_ADMIN_KEY` holds a key with the **Management** preset.

## Pick toolsets

Ask LiteGen for [toolsets](../README.md#toolsets) in the URL, or filter on
Codex's side with `enabled_tools` and `disabled_tools`:

```toml
[mcp_servers.litegen]
url = "https://app.litegen.ai/api/mcp?toolsets=images,catalog,generations"
bearer_token_env_var = "LITEGEN_API_KEY"
tool_timeout_sec = 600
# or: enabled_tools = ["list_models", "get_model", "estimate_image_cost", "generate_image"]
```

## Long jobs

Codex allows a tool call 60 seconds by default, and some image models take
longer: keep `tool_timeout_sec = 600`. `startup_timeout_sec` (default 10)
covers connecting and needs no change.

Video and 3D generations return a job at once. `wait_for_generation` waits up
to 45 seconds by default (230 at most) and returns the result, or the
progress so far; Codex calls it again until the job finishes.

## Check it works

```bash
codex mcp list
```

```text
Name     Url                             Bearer Token Env Var  Status   Auth
litegen  https://app.litegen.ai/api/mcp  LITEGEN_API_KEY       enabled  Bearer token
```

`codex mcp list` shows the configuration; it does not connect. Start
`codex`, type `/mcp` to see the server's tools, and ask: **"List LiteGen's
image models."** Codex calls `list_models` with `media_type: image`.

## OAuth

Instead of a key, Codex can sign in with your LiteGen account. Add the
server with no token, then log in:

```bash
codex mcp add litegen --url https://app.litegen.ai/api/mcp
codex mcp login litegen
```

Codex opens LiteGen's sign-in page in your browser: pick the organization,
the app and the scopes, and approve. Codex identifies itself with a client
metadata document when LiteGen offers it, as LiteGen does, and listens for
the answer on `127.0.0.1`; it checks that the answer comes from LiteGen
(`iss`). On a machine with no browser, `codex mcp login litegen
--no-browser` prints the sign-in URL: open it anywhere, approve, and paste
the URL your browser ends up on back into Codex. `codex mcp logout litegen`
forgets the tokens. To cut Codex off on LiteGen's side, revoke it under
**Connected apps** on the dashboard's Connect AI tools page.

## Troubleshooting

- **401 or "needs authentication"**: Codex reads `LITEGEN_API_KEY` from its
  own environment. An IDE or the Codex app started from the Dock, Start menu
  or a launcher may not see a variable exported in your shell profile.
  Start it from a terminal where the variable is set, or set the variable
  system-wide.
- **The server is ignored in a project**: a project's `.codex/config.toml`
  loads only in trusted projects. `~/.codex/config.toml` always loads.
- **Image generation times out**: raise `tool_timeout_sec`.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://learn.chatgpt.com/docs/extend/mcp.md> and
<https://learn.chatgpt.com/docs/config-file/config-reference.md> on
2026-11-02, and with codex-cli 0.166.0 against a local LiteGen server:
`codex mcp add --url … --bearer-token-env-var LITEGEN_API_KEY` wrote the
entry above and `codex mcp list` showed it. Codex has no command that
connects without a signed-in session; the connection itself was checked
with the [smoke script](../../../scripts/mcp-smoke/README.md), which sends
the same header.

OAuth checked on 2026-11-03 with codex-cli 0.166.0 against a local LiteGen
server: `codex mcp login litegen --no-browser` signed in with Codex's client
metadata document (`https://chatgpt.com/oauth/codex/client.json`, which
LiteGen fetched), and again with `--oauth-client-registration dcr`, which
registers instead. Each time `codex mcp list` showed `OAuth`, and the access
token Codex kept listed 22 tools on `/mcp` and called `list_models`.
