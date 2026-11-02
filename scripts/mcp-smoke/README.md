# mcp-smoke

Smoke-tests a LiteGen MCP endpoint with the official MCP TypeScript SDK
(`@modelcontextprotocol/sdk`), the way an MCP client would:

1. connects over Streamable HTTP, sending `Authorization: Bearer $LITEGEN_API_KEY`;
2. lists the tools the key can use and prints how many;
3. calls `list_models` with `media_type: image`, or `get_my_limits` on
   `/admin/mcp`, which serves no `list_models`;
4. with `--generate <model>`, calls `generate_image` with that model and
   checks an image comes back.

It exits 0 when every step succeeds and 1 on any failure, so it can gate a
deploy.

## Run

```bash
npm --prefix scripts/mcp-smoke install
LITEGEN_MCP_URL=http://127.0.0.1:4000/mcp LITEGEN_API_KEY=sk_live_… npm run mcp:smoke
LITEGEN_MCP_URL=http://127.0.0.1:4000/mcp LITEGEN_API_KEY=sk_live_… npm run mcp:smoke -- --generate mock/visual-image-gen
```

| Variable | Default | |
|---|---|---|
| `LITEGEN_MCP_URL` | `http://127.0.0.1:4000/mcp` | The endpoint: `/mcp`, `/admin/mcp`, or either with `?toolsets=` |
| `LITEGEN_API_KEY` | unset | Sent as the bearer token; leave it unset only for a dev server with no master key |

`--generate` spends the key's budget on a real model. On a local server,
enable the `mock` provider (`providers: { mock: {} }` in `litegen.yaml`) and
use `mock/visual-image-gen`, which costs nothing. `generate_image` gets ten
minutes; the SDK's default is 60 seconds.

Output against a local dev server with the mock provider:

```text
connected  http://127.0.0.1:4000/mcp (litegen 0.1.0)
tools      21
models     99 image models, e.g. bfl/flux-2-klein-9b
generated  1 image(s) with mock/visual-image-gen, 1 preview(s)
ok
```

See [docs/mcp](../../docs/mcp/README.md) for connecting real clients, and
[MCP Inspector](../../docs/mcp/clients/mcp-inspector.md) for poking at the
server by hand.
