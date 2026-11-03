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
  oauth:
    enabled: true        # let clients sign in instead of sending a key (see OAuth below)
    consent_url: null    # default: the origin of public_base_url + /oauth/consent
    access_token_ttl_secs: 3600
    refresh_token_ttl_days: 30
    rate_limit_per_minute: 30   # sign-in requests per address a minute; 0 turns the limit off
```

`server.public_base_url` matters to MCP in four ways: it is the host MCP
accepts in the `Host` header, an `Origin` MCP accepts from browsers, the
base of the file URLs tools return, and the OAuth issuer.

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
- **Pass the visitor's address in `X-Forwarded-For`.** When the proxy is on
  the same machine or a private network, LiteGen counts each sign-in request
  against the last address in that header (see [OAuth](#oauth)); without
  it, every visitor shares the proxy's allowance. Caddy sets it by default;
  nginx needs the line below.

Caddy, as LiteGen's own deployment uses (`deploy/Caddyfile`):

```caddyfile
app.example.com {
    handle /.well-known/oauth-* {
        reverse_proxy litegen:4000    # OAuth discovery, at the root and not stripped (see OAuth)
    }
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
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_buffering off;
    proxy_read_timeout 300s;
    client_max_body_size 16m;
}

location /.well-known/oauth- {
    proxy_pass http://litegen:4000;    # no trailing slash: the path goes through whole
    proxy_set_header Host $host;
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
  key, the master key, or an OAuth access token, as
  `Authorization: Bearer …`. Without them MCP answers 401 with a
  `WWW-Authenticate` header that points clients at the OAuth discovery
  documents.
- OAuth access tokens work only on the MCP endpoint they were issued for,
  never on the HTTP API.
- Session cookies are never accepted on the MCP endpoints, so a web page
  cannot drive a signed-in browser through them.
- In single-tenant mode with no master key (local development), MCP is open,
  like the rest of the API. Don't expose that to a network.

## OAuth

LiteGen is its own OAuth 2.1 authorization server, so clients can sign in
with a LiteGen account instead of sending an API key; ChatGPT and Claude
need this. It is on by default (`mcp.oauth.enabled`), and needs four things
from the deployment.

**A public HTTPS `server.public_base_url`.** It is the issuer, and each
endpoint's resource URL is built on it; clients check both against the URL
they connected to. Left unset, the issuer is `http://<host>:<port>`, which
only clients on the same machine can use. ChatGPT and Claude call LiteGen
from their own servers. If a firewall limits who reaches LiteGen, let their
egress ranges ([ChatGPT](clients/chatgpt.md), [Claude](clients/claude-apps.md))
reach the MCP endpoints, the discovery documents and `/oauth/*`.

**The discovery documents at the root of the domain.** Clients find the
authorization server from the endpoint's URL (RFC 9728 and RFC 8414), and
for a `public_base_url` with a path, the documents' URLs start at the
domain's root. With `public_base_url: https://app.example.com/api`, they are
`https://app.example.com/.well-known/oauth-protected-resource/api/mcp` and
`https://app.example.com/.well-known/oauth-authorization-server/api`. Route
`/.well-known/oauth-*` to LiteGen without stripping it, as the
[proxy examples](#behind-a-reverse-proxy) do. LiteGen answers at every path
a client may derive, so the same documents also work under `/api`.

**People who can sign in to the dashboard.** The sign-in page is the
dashboard's: whoever approves a client signs in there, with a password or a
social login. A deployment where nobody can sign in has nobody to approve.

**The consent page.** LiteGen sends people to `/oauth/consent` on the
origin of `public_base_url`. If the dashboard is served from another origin,
set `mcp.oauth.consent_url` to its consent page. The page grants access, so
no other site may frame it: serve it with
`Content-Security-Policy: frame-ancestors 'none'`, as `deploy/Caddyfile`
does.

### What clients do

- **Register on their own**, with dynamic client registration
  (`/oauth/register`) or a client ID metadata document: an HTTPS URL used as
  the client ID, which LiteGen fetches and keeps for a day. LiteGen fetches
  those documents, and the keys of clients that sign their requests, only
  from public addresses, so the server needs outbound HTTPS to the clients'
  hosts (`claude.ai` and `chatgpt.com` among them).
- **Sign their token requests**, when their metadata document says
  `private_key_jwt`, as ChatGPT's does. LiteGen checks every such request's
  signed JWT (RS256, PS256, ES256 or EdDSA) against the keys the document
  publishes, and accepts each one once.
- **Use PKCE (S256)**, with redirect URIs on HTTPS, or HTTP on `localhost`,
  `127.0.0.1` or `[::1]`, where any port matches.
- **Call from browsers.** The discovery documents, `/oauth/register`,
  `/oauth/token` and `/oauth/revoke` answer CORS from any origin, so a client
  running in a browser (the MCP Inspector) can sign in. None of them reads
  a cookie.

**Sign-in requests are limited per address.** Anyone may call
`/oauth/register` and `/oauth/authorize`, so each address may make
`mcp.oauth.rate_limit_per_minute` of those requests (30) and gets them back
over a minute; more get a 429 with `Retry-After`. IPv6 addresses count by
their /64. Behind a proxy on the same machine or a private network, the
address is the last one in `X-Forwarded-For` (see
[behind a reverse proxy](#behind-a-reverse-proxy)). Tokens, revocation and
discovery are not limited.

### Tokens and revoking

- Access tokens last `access_token_ttl_secs` (an hour) and refresh tokens
  `refresh_token_ttl_days` (30 days). Every refresh replaces the refresh
  token, and a replaced one presented again ends the sign-in. Authorization
  codes last a minute and work once; an unanswered sign-in expires after ten
  minutes.
- LiteGen stores only hashes of codes and tokens, and deletes them a day
  after they expire. An hourly cleanup also deletes the sign-ins of deleted
  organizations and apps, registrations never used within 30 days or unused
  for a year with no sign-in left (the app registers again when it next
  connects), and its copies of metadata documents once nothing uses them.
- People revoke their own sign-ins under **Connected apps** on the
  dashboard's Connect AI tools page. The master key can list anyone's with
  `GET /v1/oauth/grants?user_id=…` and revoke any with
  `DELETE /v1/oauth/grants/{id}`.
- `mcp.oauth.enabled: false` turns sign-in off and stops every OAuth token
  at once. API keys keep working.

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
