# MCP Inspector

The [MCP Inspector](https://github.com/modelcontextprotocol/inspector) is the
MCP project's tool for poking at a server: a web UI, an interactive terminal
UI, and a scriptable CLI. Use it to see exactly which tools a key gets, call
one by hand, or check LiteGen from a CI job.

## Before you start

- Node.js 22.19 or later. Inspector 2.x changed the CLI flags of 1.x; the
  commands here are for 2.x.
- A LiteGen API key from the dashboard's **Keys** page: the **AI** preset for
  `/mcp`, the **Management** preset for `/admin/mcp`. See
  [permissions](../permissions.md).
- `export LITEGEN_API_KEY=sk_live_…`

## Connect with an API key

**CLI.** List the tools your key sees:

```bash
npx -y @modelcontextprotocol/inspector --cli \
  --transport http --server-url https://app.litegen.ai/api/mcp \
  --header "Authorization: Bearer $LITEGEN_API_KEY" \
  --method tools/list --format json
```

`--method initialize` connects and prints the server's name, protocol
version and instructions without calling anything. Call a tool with
`--tool-args-json`, which passes the arguments exactly as written:

```bash
npx -y @modelcontextprotocol/inspector --cli \
  --transport http --server-url https://app.litegen.ai/api/mcp \
  --header "Authorization: Bearer $LITEGEN_API_KEY" \
  --method tools/call --tool-name list_models \
  --tool-args-json '{"media_type":"image"}' --format json
```

**Web UI.** `npx -y @modelcontextprotocol/inspector` opens it in your browser.
Choose the Streamable HTTP transport, enter the URL, add the header
`Authorization` with the value `Bearer sk_live_…`, and connect. The Tools tab
lists every tool with its input form.

**Self-hosted:** use `{public_base_url}/mcp`, for example
`https://litegen.example.com/mcp`, or `http://127.0.0.1:4000/mcp` for a
server on your machine.

## Add the admin endpoint

Point `--server-url` at `https://app.litegen.ai/api/admin/mcp` and send a
Management key. Comparing `tools/list` with an AI key and with a Management
key is the quickest way to see the split described in
[permissions](../permissions.md).

## Pick toolsets

Toolsets are part of the URL, so the Inspector shows exactly what a client
asking for them gets:

```bash
--server-url "https://app.litegen.ai/api/mcp?toolsets=images,catalog"
```

## Long jobs

Video and 3D generations return a job at once. Call `wait_for_generation`
with its `id`; it waits up to `timeout_seconds` (default 45, at most 230)
and returns the result or the progress so far.

## Check it works

`tools/list` with an AI key lists 21 tools (22 for a signed-in user, who
also gets `get_current_user`); `tools/call list_models` returns the models
your key may use.

## Check LiteGen in CI

The CLI exits non-zero on failure, with a one-line JSON error on stderr:
`3` when LiteGen refuses the credentials, `4` when it can't be reached, `5`
when the tool call fails. `--strict` adds a schema portability check of every
tool's input schema, and exits `6` on an error-level finding; LiteGen's tools
have none. Its warnings point at free-form fields such as `extra` and
`metadata`, which accept any JSON on purpose.

The repository's own smoke test does the same with the MCP TypeScript SDK:
see [`scripts/mcp-smoke`](../../../scripts/mcp-smoke/README.md).

## OAuth

Instead of a key, the Inspector can sign in with your LiteGen account.

**Web UI.** Add LiteGen's URL with no `Authorization` header and click
**Connect**. LiteGen's sign-in page opens: pick the organization, the app
and the scopes, and approve. **Clear OAuth state and disconnect** (Server
Settings → Authorization) also revokes the sign-in on LiteGen's side, as
long as **Revoke tokens on clear** stays checked.

**CLI.** With no `--header`, a 401 starts the same sign-in from a terminal:
the Inspector opens your browser and waits for the answer on
`http://127.0.0.1:6276/oauth/callback`. Without a terminal, as in CI, it
exits `3` with `auth_required` instead of waiting; `--stored-auth-only`
makes that explicit. Jobs that run unattended should send an API key.

## Troubleshooting

- **Exit `3`, `auth_required`**: LiteGen answered 401. The key is wrong,
  revoked or blocked, or `$LITEGEN_API_KEY` was empty.
- **Exit `4`**: the URL is wrong or LiteGen is down. The hosted URL includes
  `/api`.
- **A number in `--tool-arg key=value` arrives as a number**: use
  `--tool-args-json` for ids and other strings made of digits.
- More in [troubleshooting](../troubleshooting.md).

## Checked against

<https://github.com/modelcontextprotocol/inspector> (README and
`docs/cli-smoke-testing.md`) on 2026-11-02, and with Inspector 2.13.0
against a local LiteGen server: the `tools/list`, `tools/call` and
`initialize` commands above, `--strict` (no errors on either endpoint), and a
wrong key (exit `3`). The OAuth section was checked against
`docs/cli-smoke-testing.md` and `docs/test-servers.md` on 2026-11-03; the
Inspector's sign-in was not run against LiteGen, but it uses the MCP
TypeScript SDK's OAuth client, which
[`scripts/mcp-smoke/oauth-e2e.mjs`](../../../scripts/mcp-smoke/README.md)
runs against LiteGen end to end.
