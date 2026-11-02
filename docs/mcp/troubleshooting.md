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

## Tools

**A tool I expected is missing.**
- The key lacks the scope. An AI key (`generate`) has no management tools;
  `read` and `admin` add them. See [permissions](permissions.md).
- It's on the other endpoint: generation is on `/mcp`, management on
  `/admin/mcp`.
- `?toolsets=` in the URL leaves it out.
- Organization, app and member tools need a signed-in user, never a key.
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
  counts as one request.

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
it again with the same id; the job keeps running on LiteGen in between.

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
