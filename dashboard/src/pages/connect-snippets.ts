// What the Connect AI tools page shows for each client: the setup from that
// client's page in docs/mcp/clients/, with this deployment's URLs filled in.
// A unit test checks every code snippet against its docs page, so the two
// never drift apart.
import type { ScopePreset } from './key-scopes';

export type ClientId = 'claude-code' | 'claude' | 'chatgpt' | 'cursor' | 'vscode' | 'codex' | 'windsurf' | 'gemini';

export interface Snippet {
  label: string;
  language: 'bash' | 'json' | 'toml' | 'text';
  code: string;
}

/** The deployment's MCP endpoints; `management` only when `/admin/mcp` is served. */
export interface McpUrls {
  ai: string;
  management?: string;
}

/** The clients the page has a tab for, and their docs page. */
export const CLIENTS: { id: ClientId; label: string; doc: string }[] = [
  { id: 'claude-code', label: 'Claude Code', doc: 'claude-code.md' },
  { id: 'claude', label: 'Claude apps', doc: 'claude-apps.md' },
  { id: 'chatgpt', label: 'ChatGPT', doc: 'chatgpt.md' },
  { id: 'cursor', label: 'Cursor', doc: 'cursor.md' },
  { id: 'vscode', label: 'VS Code', doc: 'vscode.md' },
  { id: 'codex', label: 'Codex', doc: 'codex.md' },
  { id: 'windsurf', label: 'Windsurf', doc: 'windsurf.md' },
  { id: 'gemini', label: 'Gemini CLI', doc: 'gemini-cli.md' },
];

const API_KEY = 'Bearer ${env:LITEGEN_API_KEY}';
const ADMIN_KEY = 'Bearer ${env:LITEGEN_ADMIN_KEY}';

/** An `mcpServers` file with LiteGen, and the admin endpoint when given; `urlKey` is the client's name for the URL field. */
function mcpServers(urlKey: 'url' | 'serverUrl', urls: McpUrls): string {
  const admin = urls.management
    ? `,\n    "litegen-admin": {\n      "${urlKey}": "${urls.management}",\n      "headers": { "Authorization": "${ADMIN_KEY}" }\n    }`
    : '';
  return `{\n  "mcpServers": {\n    "litegen": {\n      "${urlKey}": "${urls.ai}",\n      "headers": { "Authorization": "${API_KEY}" }\n    }${admin}\n  }\n}`;
}

function geminiServers(urls: McpUrls): string {
  const admin = urls.management
    ? `,\n    "litegen-admin": {\n      "httpUrl": "${urls.management}",\n      "headers": { "Authorization": "Bearer $LITEGEN_ADMIN_KEY" }\n    }`
    : '';
  return `{\n  "mcpServers": {\n    "litegen": {\n      "httpUrl": "${urls.ai}",\n      "headers": { "Authorization": "Bearer $LITEGEN_API_KEY" },\n      "timeout": 600000\n    }${admin}\n  }\n}`;
}

function vscodeServers(urls: McpUrls): string {
  if (!urls.management) {
    return `{\n  "inputs": [\n    {\n      "type": "promptString",\n      "id": "litegen-api-key",\n      "description": "LiteGen API key (AI preset)",\n      "password": true\n    }\n  ],\n  "servers": {\n    "litegen": {\n      "type": "http",\n      "url": "${urls.ai}",\n      "headers": { "Authorization": "Bearer \${input:litegen-api-key}" }\n    }\n  }\n}`;
  }
  return `{\n  "inputs": [\n    { "type": "promptString", "id": "litegen-api-key", "description": "LiteGen API key (AI preset)", "password": true },\n    { "type": "promptString", "id": "litegen-admin-key", "description": "LiteGen API key (Management preset)", "password": true }\n  ],\n  "servers": {\n    "litegen": {\n      "type": "http",\n      "url": "${urls.ai}",\n      "headers": { "Authorization": "Bearer \${input:litegen-api-key}" }\n    },\n    "litegenAdmin": {\n      "type": "http",\n      "url": "${urls.management}",\n      "headers": { "Authorization": "Bearer \${input:litegen-admin-key}" }\n    }\n  }\n}`;
}

/** The setup for one client: commands and config with `$LITEGEN_API_KEY` for the key, or sign-in steps. */
export function snippets(urls: McpUrls, client: ClientId): Snippet[] {
  const { ai, management } = urls;
  const withAdmin = (snippet: Snippet): Snippet[] => (management ? [snippet] : []);
  switch (client) {
    case 'claude-code':
      return [
        {
          label: 'Add LiteGen. Claude Code reads LITEGEN_API_KEY from your environment each time it connects.',
          language: 'bash',
          code: `claude mcp add-json --scope user litegen \\\n  '{"type":"http","url":"${ai}","headers":{"Authorization":"Bearer \${LITEGEN_API_KEY}"},"timeout":600000}'`,
        },
        {
          label: 'Or let your shell put the key itself into ~/.claude.json:',
          language: 'bash',
          code: `claude mcp add --transport http --scope user litegen ${ai} \\\n  --header "Authorization: Bearer $LITEGEN_API_KEY"`,
        },
        ...withAdmin({
          label: 'The admin endpoint, with a Management key:',
          language: 'bash',
          code: `export LITEGEN_ADMIN_KEY=sk_live_…   # Management preset\nclaude mcp add-json --scope user litegen-admin \\\n  '{"type":"http","url":"${management}","headers":{"Authorization":"Bearer \${LITEGEN_ADMIN_KEY}"}}'`,
        }),
      ];
    case 'codex':
      return [
        {
          label: 'In ~/.codex/config.toml (Codex reads the key from LITEGEN_API_KEY):',
          language: 'toml',
          code: `[mcp_servers.litegen]\nurl = "${ai}"\nbearer_token_env_var = "LITEGEN_API_KEY"\ntool_timeout_sec = 600\ndefault_tools_approval_mode = "writes"`,
        },
        { label: 'Or from a terminal:', language: 'bash', code: `codex mcp add litegen --url ${ai} --bearer-token-env-var LITEGEN_API_KEY` },
        ...withAdmin({
          label: 'The admin endpoint, with a Management key in LITEGEN_ADMIN_KEY:',
          language: 'toml',
          code: `[mcp_servers.litegen-admin]\nurl = "${management}"\nbearer_token_env_var = "LITEGEN_ADMIN_KEY"\ndefault_tools_approval_mode = "writes"`,
        }),
      ];
    case 'cursor':
      return [
        { label: 'In ~/.cursor/mcp.json (Cursor reads the key from LITEGEN_API_KEY):', language: 'json', code: mcpServers('url', { ai }) },
        ...withAdmin({ label: 'With the admin endpoint, using a Management key in LITEGEN_ADMIN_KEY:', language: 'json', code: mcpServers('url', urls) }),
      ];
    case 'windsurf':
      return [
        { label: 'In ~/.codeium/windsurf/mcp_config.json (Windsurf reads the key from LITEGEN_API_KEY):', language: 'json', code: mcpServers('serverUrl', { ai }) },
        ...withAdmin({ label: 'With the admin endpoint, using a Management key in LITEGEN_ADMIN_KEY:', language: 'json', code: mcpServers('serverUrl', urls) }),
      ];
    case 'gemini':
      return [
        { label: 'In ~/.gemini/settings.json (Gemini CLI reads the key from LITEGEN_API_KEY):', language: 'json', code: geminiServers({ ai }) },
        {
          label: 'Or from a terminal:',
          language: 'bash',
          code: `gemini mcp add --scope user --transport http \\\n  --header "Authorization: Bearer $LITEGEN_API_KEY" \\\n  litegen ${ai}`,
        },
        ...withAdmin({ label: 'With the admin endpoint, using a Management key in LITEGEN_ADMIN_KEY:', language: 'json', code: geminiServers(urls) }),
      ];
    case 'vscode':
      return [
        { label: 'MCP: Open User Configuration, then add (VS Code asks for the key once and keeps it in its secret storage):', language: 'json', code: vscodeServers({ ai }) },
        {
          label: 'Or from a terminal, with the key in LITEGEN_API_KEY:',
          language: 'bash',
          code: `code --add-mcp '{"name":"litegen","type":"http","url":"${ai}","headers":{"Authorization":"Bearer \${env:LITEGEN_API_KEY}"}}'`,
        },
        ...withAdmin({ label: 'With the admin endpoint:', language: 'json', code: vscodeServers(urls) }),
      ];
    case 'chatgpt':
      return [
        {
          label: 'ChatGPT signs in with OAuth; it cannot send an API key.',
          language: 'text',
          code: `1. In ChatGPT, add a custom MCP server (connector) with this URL and OAuth sign-in:\n   ${ai}\n2. LiteGen's sign-in page opens: pick the organization and what ChatGPT may do, then approve.\n3. In a chat, @-mention LiteGen to use it.`,
        },
        ...withAdmin({
          label: 'For management, add a second server:',
          language: 'text',
          code: `Add ${management} the same way and approve the admin scopes.`,
        }),
      ];
    case 'claude':
      return [
        {
          label: 'Claude (desktop, web and mobile) signs in with OAuth.',
          language: 'text',
          code: `1. In Claude, open Customize → Connectors → Add custom connector and paste:\n   ${ai}\n2. Sign in to LiteGen when asked: pick the organization and what Claude may do, then approve.`,
        },
        ...withAdmin({
          label: 'For management, add a second connector:',
          language: 'text',
          code: `Add ${management} the same way and approve the admin scopes.`,
        }),
      ];
  }
}

/** A Cursor install link: the server entry, base64 JSON (cursor.com/docs/mcp/install-links). */
export function cursorInstallLink(name: string, config: object): string {
  return `cursor://anysphere.cursor-deeplink/mcp/install?name=${encodeURIComponent(name)}&config=${encodeURIComponent(btoa(JSON.stringify(config)))}`;
}

/** A VS Code install link: the named server entry, as `code --add-mcp` takes it. */
export function vscodeInstallLink(name: string, config: object): string {
  return `vscode:mcp/install?${encodeURIComponent(JSON.stringify({ name, ...config }))}`;
}

/** Which endpoint a key preset is for: `/mcp` (ai), `/admin/mcp` (admin), or both. */
export function presetFits(preset: Exclude<ScopePreset, 'custom'>): 'ai' | 'admin' | 'both' {
  switch (preset) {
    case 'ai':
      return 'ai';
    case 'ai_management':
      return 'both';
    case 'management':
    case 'read_only':
      return 'admin';
  }
}
