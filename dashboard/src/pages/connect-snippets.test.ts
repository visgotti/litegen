import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import {
  CLIENTS,
  cursorInstallLink,
  presetFits,
  snippets,
  vscodeInstallLink,
} from './connect-snippets';

const HOSTED = { ai: 'https://app.litegen.ai/api/mcp', management: 'https://app.litegen.ai/api/admin/mcp' };

/** The fenced code blocks of a client page in docs/mcp/clients/, dedented (a block in a list is indented). */
function codeBlocks(doc: string): string[] {
  const text = readFileSync(new URL(`../../../docs/mcp/clients/${doc}`, import.meta.url), 'utf8');
  return [...text.matchAll(/^([ \t]*)```[a-z]*\n([\s\S]*?)\n\1```$/gm)].map(([, indent, body]) =>
    body
      .split('\n')
      .map(line => (line.startsWith(indent) ? line.slice(indent.length) : line))
      .join('\n'),
  );
}

describe('Connect AI tools snippets', () => {
  it('gives Claude Code the add command with the key from the environment', () => {
    const codes = snippets({ ai: 'https://h/api/mcp' }, 'claude-code').map(s => s.code);
    expect(codes).toContain(
      'claude mcp add --transport http --scope user litegen https://h/api/mcp \\\n  --header "Authorization: Bearer $LITEGEN_API_KEY"',
    );
  });

  it('gives Codex its bearer_token_env_var setting', () => {
    const toml = snippets({ ai: 'https://h/api/mcp' }, 'codex').find(s => s.language === 'toml');
    expect(toml?.code).toContain('bearer_token_env_var = "LITEGEN_API_KEY"');
    expect(toml?.code).toContain('url = "https://h/api/mcp"');
  });

  it('shows the admin endpoint only when the deployment serves it', () => {
    for (const { id } of CLIENTS) {
      const without = snippets({ ai: HOSTED.ai }, id).map(s => s.code).join('\n');
      expect(without, id).not.toContain('admin/mcp');
      expect(snippets(HOSTED, id).map(s => s.code).join('\n'), id).toContain(HOSTED.management);
    }
  });

  it('gives ChatGPT and Claude sign-in steps, never an API key', () => {
    for (const id of ['chatgpt', 'claude'] as const) {
      const steps = snippets(HOSTED, id);
      expect(steps.every(s => s.language === 'text'), id).toBe(true);
      expect(steps.map(s => s.code).join('\n'), id).toContain(HOSTED.ai);
      expect(steps.map(s => s.code).join('\n'), id).not.toContain('LITEGEN_API_KEY');
    }
  });

  it('matches every client page in docs/mcp/clients exactly', () => {
    for (const { id, doc } of CLIENTS) {
      const blocks = codeBlocks(doc);
      for (const snippet of snippets(HOSTED, id).filter(s => s.language !== 'text')) {
        expect(blocks, `${id}: ${snippet.label}`).toContain(snippet.code);
      }
    }
  });

  it('builds a Cursor install link that decodes back to the server entry', () => {
    const config = { url: 'https://h/api/mcp', headers: { Authorization: 'Bearer ${env:LITEGEN_API_KEY}' } };
    const link = new URL(cursorInstallLink('litegen', config));
    expect(link.protocol).toBe('cursor:');
    expect(link.searchParams.get('name')).toBe('litegen');
    expect(JSON.parse(atob(link.searchParams.get('config')!))).toEqual(config);
  });

  it('builds a VS Code install link carrying the named server entry', () => {
    const config = { type: 'http', url: 'https://h/api/mcp', headers: { Authorization: 'Bearer ${env:LITEGEN_API_KEY}' } };
    const link = vscodeInstallLink('litegen', config);
    expect(link.startsWith('vscode:mcp/install?')).toBe(true);
    expect(JSON.parse(decodeURIComponent(link.slice('vscode:mcp/install?'.length)))).toEqual({ name: 'litegen', ...config });
  });

  it('says which endpoint each key preset suits', () => {
    expect(presetFits('ai')).toBe('ai');
    expect(presetFits('management')).toBe('admin');
    expect(presetFits('read_only')).toBe('admin');
    expect(presetFits('ai_management')).toBe('both');
  });
});
