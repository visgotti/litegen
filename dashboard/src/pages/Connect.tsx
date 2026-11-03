import { useCallback, useEffect, useState } from 'react';
import type { AuthConfigResponse, ConnectedApp } from '@litegen/sdk';
import { client, getApiKey } from '../sdk-client';
import { showToast } from '../components/toast-store';
import { permissionsFor } from '../components/permissions';
import KeyCreatedBanner from '../components/KeyCreatedBanner';
import { useTenant } from '../context/tenant';
import {
  CLIENTS,
  cursorInstallLink,
  presetFits,
  snippets,
  vscodeInstallLink,
  type ClientId,
  type McpUrls,
} from './connect-snippets';
import { PRESETS, availablePresets, type ScopePreset } from './key-scopes';
import './connect.css';

/** The MCP setup guides, one page per client. */
const DOCS_URL = 'https://github.com/litegen/litegen/tree/main/docs/mcp';
const FITS = { ai: 'For /mcp', admin: 'For /admin/mcp', both: 'For both endpoints' } as const;
/** A server entry that reads the key from LITEGEN_API_KEY, for install links. */
const ENV_KEY = { Authorization: 'Bearer ${env:LITEGEN_API_KEY}' };

type NamedPreset = Exclude<ScopePreset, 'custom'>;

async function copy(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    showToast(`Copied ${what}`, 'info');
  } catch {
    showToast('Failed to copy', 'error');
  }
}

/** Use LiteGen from AI tools: the MCP endpoints, each client's setup, a key for it, and the apps connected through OAuth. */
export default function Connect() {
  const { activeOrgRole } = useTenant();
  // Who may mint an admin-scope key: the server's rule, as on the Keys page.
  const canGrantAdmin = Boolean(getApiKey()) || permissionsFor(activeOrgRole ?? '').includes('key:write:any');
  const presets = availablePresets(canGrantAdmin).filter((p): p is NamedPreset => p !== 'custom');

  // undefined while loading; null when the server has MCP turned off.
  const [mcp, setMcp] = useState<AuthConfigResponse['mcp'] | undefined>(undefined);
  const [tab, setTab] = useState<ClientId>('claude-code');
  const [preset, setPreset] = useState<NamedPreset>('ai');
  const [keyName, setKeyName] = useState('MCP');
  const [created, setCreated] = useState<{ secret: string; publicId: string } | null>(null);
  // null when the caller has no connected apps to show (an API-key login).
  const [apps, setApps] = useState<ConnectedApp[] | null | undefined>(undefined);
  const [error, setError] = useState('');

  useEffect(() => {
    client.auth
      .config()
      .then(config => setMcp(config.mcp ?? null))
      .catch((e: Error) => setError(e.message));
  }, []);

  const loadApps = useCallback(() => {
    client.oauth
      .listGrants()
      .then(setApps)
      .catch(() => setApps(null));
  }, []);
  useEffect(loadApps, [loadApps]);

  async function createKey() {
    setError('');
    try {
      const result = await client.keys.create({ name: keyName.trim() || 'MCP', scopes: PRESETS[preset].scopes });
      setCreated({ secret: result.key, publicId: result.public_id });
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function revoke(app: ConnectedApp) {
    try {
      await client.oauth.revokeGrant(app.id);
      showToast(`Disconnected ${app.client_name}`, 'info');
      loadApps();
    } catch (e) {
      setError((e as Error).message);
    }
  }

  const urls: McpUrls | null = mcp ? { ai: mcp.ai_url, ...(mcp.admin_url ? { management: mcp.admin_url } : {}) } : null;

  return (
    <div>
      <h2 className="page-title">Connect AI tools</h2>
      <p className="connect-intro">
        Use LiteGen from Claude, ChatGPT, Cursor, VS Code, Codex and any other MCP client: generate images, video and 3D
        models, and manage keys, budgets and spend, with the same permissions as the API.{' '}
        <a href={DOCS_URL} target="_blank" rel="noreferrer">Setup guides</a>
      </p>

      {error && <div className="alert alert-error" data-testid="connect-error">{error}</div>}

      {mcp === null && (
        <div className="alert alert-error" data-testid="connect-mcp-off">
          MCP is turned off on this server (<code>mcp.enabled: false</code> in its configuration).
        </div>
      )}

      {urls && (
        <>
          <section className="connect-section">
            <h3>Endpoints</h3>
            <p>
              {mcp?.oauth
                ? 'Clients connect with an API key, or sign in with your LiteGen account (OAuth).'
                : 'Clients connect with an API key. OAuth sign-in is turned off on this server.'}
            </p>
            <div className="connect-url">
              <span className="connect-url-label">AI</span>
              <code data-testid="connect-ai-url">{urls.ai}</code>
              <button className="btn btn-secondary" onClick={() => copy(urls.ai, 'the AI endpoint')}>Copy</button>
            </div>
            {urls.management && (
              <div className="connect-url">
                <span className="connect-url-label">Management</span>
                <code data-testid="connect-admin-url">{urls.management}</code>
                <button className="btn btn-secondary" onClick={() => copy(urls.management!, 'the management endpoint')}>Copy</button>
              </div>
            )}
          </section>

          <section className="connect-section">
            <h3>Set up a client</h3>
            <p>Key-based setups read the key from <code>LITEGEN_API_KEY</code>: create one below and set it in your environment.</p>
            <div className="connect-tabs" role="tablist">
              {CLIENTS.map(c => (
                <button
                  key={c.id}
                  role="tab"
                  aria-selected={tab === c.id}
                  className={`connect-tab${tab === c.id ? ' active' : ''}`}
                  data-testid={`connect-tab-${c.id}`}
                  onClick={() => setTab(c.id)}
                >
                  {c.label}
                </button>
              ))}
            </div>
            {tab === 'cursor' && (
              <div className="connect-install">
                <a className="btn btn-secondary" data-testid="connect-install-cursor" href={cursorInstallLink('litegen', { url: urls.ai, headers: ENV_KEY })}>
                  Add to Cursor
                </a>
              </div>
            )}
            {tab === 'vscode' && (
              <div className="connect-install">
                <a
                  className="btn btn-secondary"
                  data-testid="connect-install-vscode"
                  href={vscodeInstallLink('litegen', { type: 'http', url: urls.ai, headers: ENV_KEY })}
                >
                  Install in VS Code
                </a>
              </div>
            )}
            <div data-testid="connect-snippets">
              {snippets(urls, tab).map(s => (
                <div className="connect-snippet" key={s.label}>
                  <div className="connect-snippet-head">
                    <span>{s.label}</span>
                    {s.language !== 'text' && (
                      <button className="btn btn-secondary" onClick={() => copy(s.code, 'the snippet')}>Copy</button>
                    )}
                  </div>
                  <pre><code>{s.code}</code></pre>
                </div>
              ))}
            </div>
          </section>

          <section className="connect-section">
            <h3>Create a key for MCP</h3>
            <p>Pick what the client may do. The key is shown once.</p>
            <div className="connect-presets">
              {presets.map(p => (
                <label className="connect-preset" key={p}>
                  <input
                    type="radio"
                    name="connect-preset"
                    data-testid={`connect-preset-${p}`}
                    checked={preset === p}
                    onChange={() => setPreset(p)}
                  />
                  <span className="connect-preset-text">
                    <strong>{PRESETS[p].label}</strong>
                    <span>{FITS[presetFits(p)]}</span>
                    <span>{PRESETS[p].description}</span>
                  </span>
                </label>
              ))}
            </div>
            <div className="connect-key-form">
              <input className="input" data-testid="connect-key-name" value={keyName} onChange={e => setKeyName(e.target.value)} aria-label="Key name" />
              <button className="btn btn-primary" data-testid="connect-create-key" onClick={createKey}>Create key</button>
            </div>
            {created && <div style={{ marginTop: 14 }}><KeyCreatedBanner secret={created.secret} publicId={created.publicId} /></div>}
          </section>
        </>
      )}

      <section className="connect-section">
        <h3>Connected apps</h3>
        <p>Apps you signed in to LiteGen with OAuth. Revoking one stops its access at once.</p>
        {apps === undefined ? (
          <div className="loading">Loading…</div>
        ) : apps === null ? (
          <p className="connect-small">Sign in with your account to see the apps you have connected.</p>
        ) : apps.length === 0 ? (
          <p className="connect-small" data-testid="connected-apps-empty">No apps connected yet.</p>
        ) : (
          <div className="table-container">
            <table>
              <thead>
                <tr>
                  <th>App</th>
                  <th>Endpoint</th>
                  <th>Scopes</th>
                  <th>Last used</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {apps.map(app => (
                  <tr key={app.id} data-testid="connected-app-row">
                    <td>{app.client_name}</td>
                    <td>{app.surface === 'admin' ? '/admin/mcp' : '/mcp'}</td>
                    <td>{app.scopes.join(' ')}</td>
                    <td>{app.last_used_at ? new Date(app.last_used_at).toLocaleString() : 'Never'}</td>
                    <td>
                      <button className="btn btn-danger" onClick={() => revoke(app)}>Revoke</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
