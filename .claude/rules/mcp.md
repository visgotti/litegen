---
paths:
  - "litegen-core/src/mcp/**"
  - "litegen-core/src/api/**"
  - "litegen-core/src/types/**"
  - "docs/mcp/**"
  - "deploy/**"
  - "scripts/mcp-smoke/**"
  - "dashboard/src/pages/Connect.tsx"
---

# MCP server (`/mcp`, `/admin/mcp`)

The MCP tools are built from the OpenAPI document. Every tool call runs
through the real axum router, so a tool has exactly the permissions of its
HTTP route. Keep that true when the API changes:

- **Every OpenAPI operation is a tool or is excluded.** A new route needs a row
  in `TOOLS` (`litegen-core/src/mcp/catalog.rs`) or an `EXCLUDED` entry with a
  reason. Otherwise `every_openapi_operation_is_a_tool_or_excluded` fails.
- **Regenerate the tools reference.** After you change a tool, or any OpenAPI
  text a tool shows, run this from `litegen-core/`:
  `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`. Otherwise
  `tools_reference_is_current` fails.
- **Update the tool counts together.** They're pinned in `src/mcp/tests.rs`
  (`TOOLS`, `EXCLUDED`) and `tests/mcp_api.rs` (`AI_TOOLS_FOR_KEYS`), and
  written in `docs/mcp/README.md` and `docs/mcp/clients/mcp-inspector.md`.
- **Map each header parameter to a tool argument** in `src/mcp/schema.rs`:
  - `Idempotency-Key` becomes `idempotency_key`;
  - `X-Litegen-Keep-Outputs` becomes `keep_outputs`;
  - `Prefer` is skipped, because a tool answers only when the job is done.

  If a new header parameter has no mapping, the registry fails to build
  ("unsupported header parameter") and the MCP tests panic.
- **MCP results drop response headers.** If a caller needs a header, turn it
  into a note in `notes()` (`src/mcp/content.rs`), as `X-Litegen-Cache: hit`
  already is.

## Deployment

`docs/mcp/self-hosting.md` covers what a reverse proxy needs:

- the OAuth discovery routes at the root;
- `frame-ancestors 'none'` on the consent page;
- `X-Forwarded-For`, so the sign-in limit counts each address separately.

`deploy/Caddyfile` and `deploy/Caddyfile.visgotti` have all three (the
visgotti file since 2026-11-12). Caddy's `reverse_proxy` sets
`X-Forwarded-For` itself.

## Testing with real clients

- **Live checks against a local server:** `scripts/mcp-smoke/` has
  `smoke.mjs` (API key) and `oauth-e2e.mjs` (the OAuth flow). Its README has
  the commands.
- **Where each client keeps its config and tokens, and how it signs in:**
  `docs/superpowers/research/2026-11-02-mcp-clients.md`.
- **Keep the machine's real sign-ins safe.** Point each client at a throwaway
  config: `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `HOME` for Gemini CLI,
  `MCP_REMOTE_CONFIG_DIR`, `MCP_INSPECTOR_SECRET_STORE=memory`.
- **Drive an interactive sign-in through a Python pty.** On macOS, `script`
  fails when stdin isn't a terminal. The code that approves the sign-in has to
  keep reading the pty, or the client stalls.
- **Start a fresh server for each run.** A single-tenant server lets only its
  first account sign up.

## Open question

The dashboard's Connect page links to its docs at `github.com/litegen/litegen`
(the README's URL), but the git remote is `github.com/visgotti/litegen`. Which
one is right is the user's call, so don't change either link without asking.
