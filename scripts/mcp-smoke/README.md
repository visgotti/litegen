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

## OAuth end to end

`oauth-e2e.mjs` signs in with OAuth the way ChatGPT or claude.ai does, with
the same SDK as the client:

1. signs up (or signs in) a person through the API, for a dashboard session;
2. connects with no credentials: the SDK reads the 401, finds LiteGen's
   authorization server, registers itself and asks for consent;
3. plays the person: follows the authorization URL to the consent page and
   approves through the consent API, checking `iss` on the way back;
4. hands the code to the SDK, then lists tools and calls `list_models`;
5. waits for the access token to expire and calls again: the SDK refreshes;
6. revokes the grant and checks the client is locked out.

Run it against a fresh single-tenant server with a master key and a short
access token life, so step 5 does not wait an hour:

```bash
LITEGEN__MASTER_KEY=… LITEGEN__MCP__OAUTH__ACCESS_TOKEN_TTL_SECS=3 litegen   # in another terminal
npm --prefix scripts/mcp-smoke install
npm run mcp:oauth-e2e
```

| Variable | Default | |
|---|---|---|
| `LITEGEN_BASE_URL` | `http://127.0.0.1:4000` | The server (its MCP endpoint is `/mcp` under it) |
| `LITEGEN_E2E_EMAIL`, `LITEGEN_E2E_PASSWORD` | an e2e account | Signed up on a fresh database, signed in otherwise |
| `LITEGEN_E2E_TTL_WAIT_MS` | `4000` | How long to wait for the access token to expire |

Output:

```text
- signed in to the dashboard as mcp-oauth-e2e@example.com
- registered as lgc_…; sent to http://127.0.0.1:4000/oauth/authorize
- approved ai:generate ai:read; tokens issued
- connected: 22 tools, list_models works
- the expired access token was refreshed and the call went through
- revoked: the client is locked out and must sign in again
PASS
```

See [docs/mcp](../../docs/mcp/README.md) for connecting real clients, and
[MCP Inspector](../../docs/mcp/clients/mcp-inspector.md) for poking at the
server by hand.
