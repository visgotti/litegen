# ChatGPT

ChatGPT connects to MCP servers only with OAuth: it cannot send an API key
or other custom headers. LiteGen's OAuth support is coming; this page will
cover connecting then.

Until then, OpenAI's other surfaces take an API key:

- **Codex** (CLI, IDE extension and app): see [Codex](codex.md).
- **The Responses API and the Agents SDK**: see
  [APIs and SDKs](apis-and-sdks.md#openai-responses-api).

## Checked against

<https://developers.openai.com/api/docs/guides/custom-mcp-server> on
2026-11-02.
