# Claude Desktop and claude.ai

Claude's apps add remote MCP servers as custom connectors, which sign in
with OAuth. LiteGen's OAuth support is coming; this page will cover
connecting then.

Until then:

- **Claude Code** connects with an API key today: see
  [Claude Code](claude-code.md).
- **Claude Desktop** can run LiteGen as a local server through the
  `mcp-remote` bridge: add the entry from
  [clients that only start local servers](other-clients.md#clients-that-only-start-local-servers)
  to `claude_desktop_config.json` (macOS:
  `~/Library/Application Support/Claude/claude_desktop_config.json`;
  Windows: `%APPDATA%\Claude\claude_desktop_config.json`), then restart
  Claude Desktop.
- **Organizations with custom-connector request headers** (a beta) can add
  LiteGen as a custom connector with no sign-in and the header
  `authorization` set to `Bearer sk_live_…`. Everyone who uses the
  connector then shares that key.

## Checked against

<https://claude.com/docs/connectors/custom/add-unlisted.md> and
<https://github.com/geelen/mcp-remote> on 2026-11-02.
