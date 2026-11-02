# Running LiteGen's MCP server yourself

MCP is part of `litegen-core`: every deployment serves `/mcp` and
`/admin/mcp` on its own port, with nothing else to run.

## Configuration

`litegen.yaml` (or the matching `LITEGEN__MCP__*` environment variables):

```yaml
server:
  public_base_url: "https://litegen.example.com/api"   # set this behind any proxy

mcp:
  enabled: true          # serve MCP at all; false turns both endpoints off
  admin_enabled: true    # also serve /admin/mcp (the management API)
  allowed_hosts: []      # extra Host values to accept (see below); ["*"] turns the check off
  allowed_origins: []    # extra browser Origins to accept (see below)
```

`server.public_base_url` matters to MCP in three ways: it is the host MCP
accepts in the `Host` header, an `Origin` MCP accepts from browsers, and the
base of the file URLs tools return.

The endpoints are `{public_base_url}/mcp` and `{public_base_url}/admin/mcp`.

## Behind a reverse proxy

- **Keep the `Host` header**, or add the host your proxy sends to
  `mcp.allowed_hosts`. MCP rejects any `Host` it does not know with a 403,
  which protects a server on localhost from DNS-rebinding pages. Loopback
  (`localhost`, `127.0.0.1`, `::1`) and the host of `public_base_url` are
  always accepted.
- **Do not buffer responses.** A tool that reports progress answers with a
  `text/event-stream`. LiteGen sends `X-Accel-Buffering: no` and
  `Cache-Control: no-cache` on those responses, which nginx honors; other
  proxies need buffering off for the MCP paths.
- **Allow long requests.** `wait_for_generation` holds a request open for up
  to 230 seconds, and image generation can take minutes. Proxy read timeouts
  under that cut calls off.
- **Allow 16 MiB bodies** on the MCP paths if your agents send meshes to
  `convert_3d_model` as base64.

Caddy, as LiteGen's own deployment uses (`deploy/Caddyfile`):

```caddyfile
app.example.com {
    handle_path /api/* {
        reverse_proxy litegen:4000    # Caddy keeps Host and streams responses by default
    }
}
```

nginx:

```nginx
location /api/ {
    proxy_pass http://litegen:4000/;
    proxy_set_header Host $host;
    proxy_buffering off;
    proxy_read_timeout 300s;
    client_max_body_size 16m;
}
```

## Browsers and the Origin check

Non-browser clients (Claude Code, Codex, IDEs, SDKs) send no `Origin` and
are not affected. A request that does carry an `Origin` is accepted only
from:

- the dashboard's CORS origins (`cors.allowed_origins` or
  `LITEGEN_CORS_ORIGINS`);
- the origin of `public_base_url`;
- `mcp.allowed_origins`.

Any other `Origin` gets a 403. A CORS list containing `"*"` accepts every
origin; don't use that on a deployment reachable from the internet.

## Authentication

- With a master key configured, every MCP request needs credentials: an API
  key or the master key as `Authorization: Bearer …`. Without them MCP
  answers 401 with a `WWW-Authenticate` header.
- Session cookies are never accepted on the MCP endpoints, so a web page
  cannot drive a signed-in browser through them.
- In single-tenant mode with no master key (local development), MCP is open,
  like the rest of the API. Don't expose that to a network.

## Turning endpoints off

- `mcp.enabled: false`: no MCP at all (both paths 404).
- `mcp.admin_enabled: false`: `/mcp` only. Agents can still generate, but
  the management API is not offered as MCP, whatever a key's scopes.

## Observability

- `GET /metrics` exports `litegen_mcp_requests_total{surface, method}` (every
  MCP request, by JSON-RPC method) and
  `litegen_mcp_tool_calls_total{tool, surface, outcome}` (every tool call,
  `ok` or `error`).
- Tracing spans `mcp.request` and `mcp.tool` carry the method or tool, the
  endpoint and how the caller authenticated.
- Every generation made through MCP carries the spend tag `mcp`.
