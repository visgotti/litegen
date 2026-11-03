# Troubleshooting LiteGen MCP

Each entry: what you see, why, and what to do.

## Connecting

**401 Unauthorized, or the client says it needs authentication.**
The request had no usable credentials.
- Check the header is exactly `Authorization: Bearer sk_live_…`.
- Check the environment variable reached the client. Claude Code sends a
  literal `${LITEGEN_API_KEY}` when the variable is unset; Codex reads the
  variable named in `bearer_token_env_var` from its own environment, so an
  IDE launched from the dock may not see a variable exported in your shell.
- The key may be revoked, expired or blocked. Check the Keys page.
- With no header at all, a client that supports OAuth offers to sign in
  instead; see [OAuth](#oauth).

**402 with `quota_exceeded` when connecting.** The key's lifetime quota is
spent. Raise or remove it on the Keys page.

**403 "Host header is not allowed".** Your reverse proxy sends a `Host`
LiteGen does not know. Pass the original `Host` through, set
`server.public_base_url` to the public URL, or add the host to
`mcp.allowed_hosts`. See [self-hosting](self-hosting.md).

**403 `forbidden_origin`.** A browser-based client sent an `Origin` that is
not in the CORS origins, `public_base_url` or `mcp.allowed_origins`.

**406 Not Acceptable.** The client sent `Accept: application/json` alone.
The Streamable HTTP transport requires `Accept: application/json,
text/event-stream`. Every mainstream client sends both; a hand-written
client must too.

**400 `unknown_toolset`.** A name in `?toolsets=` is not one of the
endpoint's toolsets. The error lists the valid names.

**400 on every request from an old client.** The client sent an
`MCP-Protocol-Version` LiteGen does not support. LiteGen supports
2024-11-05 through 2026-07-28; update the client.

**404 on `/mcp` or `/admin/mcp`.** The endpoint is turned off (`mcp.enabled`
or `mcp.admin_enabled`), or the URL is missing the `/api` prefix your
deployment serves the API under.

## OAuth

**The client never offers to sign in, or says the server has no OAuth.** It
found no discovery documents.
- On a self-hosted server behind a path prefix, route the root
  `/.well-known/oauth-*` to LiteGen; see [self-hosting](self-hosting.md#oauth).
  `curl https://app.example.com/.well-known/oauth-protected-resource/api/mcp`
  should print JSON.
- `mcp.oauth.enabled` is `false`: the 401 then says only
  `Bearer realm="litegen"`.

**The client says the issuer or the resource doesn't match.**
`server.public_base_url` is not the address the client connected to. Set it
to the public HTTPS URL, prefix included (`https://app.example.com/api`).

**"Can't sign in to LiteGen: This app's sign-in link is invalid".** LiteGen
could not trust the request enough to send you back to the app: it doesn't
know the client, the client's metadata document could not be fetched, or the
redirect URI is not one the client declared. Connect again from the app. If
it repeats, remove LiteGen from the app and add it again, so the app
registers afresh. A self-hosted server must be able to reach the client's
metadata URL over HTTPS (on `claude.ai` or `chatgpt.com`, for example).

**"This sign-in request has expired. Go back to the app and connect
again."** A sign-in must be answered within ten minutes, and only once.
Start it again from the app.

**"Your role in your organizations can't grant what this app asks for."**
The app asked only for scopes your role can't approve, such as `admin:write`
as a member. Ask an owner or admin to connect it, or have the app ask for
`admin:read`.

**The app reports `access_denied`.** Someone chose **Deny** on the sign-in
page. Connect again to approve.

**The app reports `invalid_target`.** It asked to sign in for a `resource`
that is not one of LiteGen's MCP endpoints, usually because
`server.public_base_url` differs from the URL the app uses.

**The app reports `invalid_scope`.** It asked for `ai:` and `admin:` scopes
at once (one sign-in is for one endpoint: connect each endpoint on its own),
for `admin:` scopes on a server without `/admin/mcp`, or, when refreshing,
for more than you approved.

**401 `invalid_token` after signing in.** LiteGen no longer accepts the
access token. The client should refresh it or sign in again; the message
says why:
- "the access token is unknown, expired or revoked": access tokens last an
  hour, and clients refresh them on their own. When the refresh fails too,
  the sign-in itself has ended; see the next entry.
- "this sign-in is for the other MCP endpoint": a `/mcp` sign-in was used on
  `/admin/mcp`, or the other way round. Sign in to each endpoint.
- "the person who approved this app can no longer act in its organization":
  you left the organization, or your account was deactivated.
- "the app this sign-in was approved for no longer exists": the app picked on
  the sign-in page was deleted. Sign in again and pick another.

**The client keeps asking to sign in again (`invalid_grant` on refresh).**
The refresh token stopped working. The message says why:
- "the app's access was revoked": someone revoked it under **Connected
  apps**, or the app revoked it itself.
- "the person who approved this app can no longer use LiteGen there": you
  left the organization, or your account was deactivated.
- "the refresh token is expired or revoked": refresh tokens last 30 days by
  default.
- "the refresh token was used before; the app's access is revoked": a
  refresh token was used twice, so LiteGen ended the sign-in in case the
  token was stolen.

**Signed in, but a tool is missing.** The scopes you approved don't open it
(`ai:read` alone can't generate, `admin:read` can't change anything), or
your role can't use it. Sign in again and approve other scopes; see
[permissions](permissions.md#oauth).

## Tools

**A tool I expected is missing.**
- The key lacks the scope. An AI key (`generate`) has no management tools;
  `read` and `admin` add them. See [permissions](permissions.md).
- It's on the other endpoint: generation is on `/mcp`, management on
  `/admin/mcp`.
- `?toolsets=` in the URL leaves it out.
- Organization, app and member tools need a signed-in user, never a key.
- Signed in with OAuth: the scopes you approved leave it out (see
  [OAuth](#oauth)).
- The hosted master key belongs to no organization, so on `/mcp` it sees
  only the model catalog.
- The client lists only a subset (Claude Code's tool search loads tools on
  demand; some clients cap the count). Use toolsets to narrow the list.

**`Unknown tool: …`.** The credential cannot call that tool (as above), or
the name is misspelled.

**`HTTP 4xx …` tool errors.** These are LiteGen's own answers: the same
status, code and message the HTTP API gives. Common ones:
- `budget_exceeded` (402): a budget on the key, org, app, member or end user
  is spent. `get_my_limits` shows which.
- `model_not_allowed` (403): the org or app restricts models.
- `invalid_request_error` (400): a parameter is wrong; `get_model` shows the
  model's parameters.
- `rate_limit_exceeded` (429): the key's per-minute limit. Each tool call
  counts as one request, and each status read `wait_for_generation` makes
  counts too. It paces itself to use at most half the key's limit and sits
  out a 429 instead of failing.

**"LiteGen ignored these parameters".** The call used `strict: false` and
the model doesn't take those parameters. Drop them, or check `get_model`.

**Results end with `_truncated`.** The full result was over 100,000
characters. Use the tool's paging or filter arguments; for models, filter
`list_models` by `media_type` and read one with `get_model`.

## Long jobs

**The client times out during image generation.** Some image models take a
minute or more, and clients default to short limits: Claude Code allows 60 s
per HTTP request, Codex 60 s per tool call, and OpenAI's Agents SDK only 5 s
per session request. Raise the limit (each client page shows how) to at
least 600 s.

**`wait_for_generation` keeps returning "Still processing".** It waits at
most `timeout_seconds` (default 45, max 230) so it fits client limits. Call
it again with the same id; the job keeps running on LiteGen in between. It
reads the job's state at once, then again after two seconds, and the gap
grows to ten. A key limited to N requests a minute reads at most N/2 times a
minute: at 6 a minute, every 20 seconds. So a tightly limited key can get
"Still processing" after a single read.

**No progress updates.** Progress needs a client that sends a progress token
and a proxy that does not buffer `text/event-stream` responses. See
[self-hosting](self-hosting.md).

## Files

**No image preview appears.** Previews are on by default for
`generate_image`; check the call didn't pass `preview: false`. A preview is
skipped (never an error) when the image can't be fetched or decoded. The
full image is always at its URL.

**"The converted file is … over MCP's 8 MiB limit".** Call
`POST {public_base_url}/v1/utils/models3d/convert` directly for large meshes.
