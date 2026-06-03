import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { ProviderCredentialInfo } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';

export default function Organization() {
  const { orgs, activeOrg, activeApp, apps, refresh } = useTenant();

  const currentOrg = orgs.find(o => o.id === activeOrg) ?? null;
  const loadedOrgName = currentOrg?.name ?? '';

  // Rename org. `seenOrgName` lets us reset the editable field when the active
  // org's name changes (without a setState-in-effect), per React docs guidance.
  const [orgNameDraft, setOrgNameDraft] = useState<string | null>(null);
  const [seenOrgName, setSeenOrgName] = useState(loadedOrgName);
  if (loadedOrgName !== seenOrgName) {
    setSeenOrgName(loadedOrgName);
    setOrgNameDraft(null);
  }
  const orgName = orgNameDraft ?? loadedOrgName;
  const setOrgName = (v: string) => setOrgNameDraft(v);

  // Create app
  const [newAppName, setNewAppName] = useState('');
  // Provider credentials
  const [creds, setCreds] = useState<ProviderCredentialInfo[]>([]);
  const [credProvider, setCredProvider] = useState('mock');
  const [credSecret, setCredSecret] = useState('');

  const loadCreds = useCallback(async () => {
    if (!activeApp) return [] as ProviderCredentialInfo[];
    try {
      return await client.apps.providerCredentials.list(activeApp);
    } catch {
      return [] as ProviderCredentialInfo[];
    }
  }, [activeApp]);

  useEffect(() => {
    let cancelled = false;
    void loadCreds().then(list => { if (!cancelled) setCreds(list); });
    return () => { cancelled = true; };
  }, [loadCreds]);

  const saveOrgName = async () => {
    if (!activeOrg || !orgName.trim()) return;
    try {
      await client.orgs.update(activeOrg, { name: orgName.trim() });
      setOrgNameDraft(null);
      await refresh();
      showToast('Organization renamed', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Rename failed', 'error');
    }
  };

  const createApp = async () => {
    if (!activeOrg || !newAppName.trim()) return;
    try {
      await client.orgs.apps.create(activeOrg, { name: newAppName.trim() });
      setNewAppName('');
      await refresh();
      showToast('Application created', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Create failed', 'error');
    }
  };

  const deleteApp = async (appId: string) => {
    try {
      await client.apps.delete(appId);
      await refresh();
      showToast('Application deleted', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Delete failed', 'error');
    }
  };

  const addCred = async () => {
    if (!activeApp || !credProvider.trim() || !credSecret.trim()) return;
    try {
      await client.apps.providerCredentials.create(activeApp, {
        provider: credProvider.trim(),
        credentials: { api_key: credSecret.trim() },
      });
      setCredSecret('');
      setCreds(await loadCreds());
      showToast('Provider credential added', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Add failed', 'error');
    }
  };

  const deleteCred = async (provider: string) => {
    if (!activeApp) return;
    try {
      await client.apps.providerCredentials.delete(activeApp, provider);
      setCreds(await loadCreds());
      showToast('Provider credential removed', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error');
    }
  };

  if (!activeOrg) {
    return (
      <div style={{ padding: 24, color: '#8b949e' }} data-testid="org-no-active">
        No active organization. Sign in to a hosted account to manage organizations.
      </div>
    );
  }

  const cardStyle: React.CSSProperties = {
    background: '#161b22',
    border: '1px solid #30363d',
    borderRadius: 10,
    padding: 20,
    marginBottom: 24,
  };
  const sectionTitle: React.CSSProperties = { margin: '0 0 16px', color: '#e6edf3', fontSize: 18, fontWeight: 600 };

  return (
    <div style={{ padding: '0 0 32px' }}>
      <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Organization</h2>

      {/* Rename org */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Name</h3>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <input
            className="input"
            data-testid="org-name-input"
            value={orgName}
            onChange={e => setOrgName(e.target.value)}
            placeholder="Organization name"
            style={{ maxWidth: 320 }}
          />
          <button className="btn btn-primary" data-testid="org-name-save" onClick={saveOrgName}>
            Save
          </button>
        </div>
      </div>

      {/* Apps */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Applications</h3>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 16 }}>
          {apps.length === 0 && (
            <div style={{ color: '#8b949e', fontSize: 14 }}>No applications yet.</div>
          )}
          {apps.map(a => (
            <div
              key={a.id}
              data-testid={`app-row-${a.id}`}
              style={{
                display: 'flex', alignItems: 'center', justifyContent: 'space-between',
                padding: '10px 14px', background: '#0d1117', border: '1px solid #30363d', borderRadius: 8,
              }}
            >
              <span style={{ color: '#e6edf3', fontSize: 14 }}>{a.name}</span>
              <button
                className="btn btn-danger"
                data-testid={`app-delete-${a.id}`}
                onClick={() => deleteApp(a.id)}
                style={{ fontSize: 12, padding: '4px 10px' }}
              >
                Delete
              </button>
            </div>
          ))}
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <input
            className="input"
            data-testid="app-create-name"
            value={newAppName}
            onChange={e => setNewAppName(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && createApp()}
            placeholder="New application name"
            style={{ maxWidth: 320 }}
          />
          <button className="btn btn-primary" data-testid="app-create-submit" onClick={createApp}>
            Create app
          </button>
        </div>
      </div>

      {/* Provider credentials */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Provider credentials</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Scoped to the active application{activeApp ? '' : ' — select an app to manage credentials'}.
        </p>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 16 }}>
          {creds.length === 0 && (
            <div style={{ color: '#8b949e', fontSize: 14 }}>No provider credentials configured.</div>
          )}
          {creds.map(c => (
            <div
              key={c.provider}
              data-testid={`provider-cred-row-${c.provider}`}
              style={{
                display: 'flex', alignItems: 'center', justifyContent: 'space-between',
                padding: '10px 14px', background: '#0d1117', border: '1px solid #30363d', borderRadius: 8,
              }}
            >
              <span style={{ color: '#e6edf3', fontSize: 14 }}>
                <strong>{c.provider}</strong>
                {c.display_hint && (
                  <span style={{ color: '#8b949e', marginLeft: 8, fontFamily: 'monospace', fontSize: 13 }}>
                    {c.display_hint}
                  </span>
                )}
              </span>
              <button
                className="btn btn-danger"
                data-testid={`provider-cred-delete-${c.provider}`}
                onClick={() => deleteCred(c.provider)}
                style={{ fontSize: 12, padding: '4px 10px' }}
              >
                Delete
              </button>
            </div>
          ))}
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
          <input
            className="input"
            data-testid="provider-cred-provider"
            value={credProvider}
            onChange={e => setCredProvider(e.target.value)}
            placeholder="provider (e.g. mock)"
            style={{ maxWidth: 180 }}
            disabled={!activeApp}
          />
          <input
            className="input"
            data-testid="provider-cred-secret"
            type="password"
            value={credSecret}
            onChange={e => setCredSecret(e.target.value)}
            placeholder="API key / secret"
            style={{ maxWidth: 280 }}
            disabled={!activeApp}
          />
          <button
            className="btn btn-primary"
            data-testid="provider-cred-add"
            onClick={addCred}
            disabled={!activeApp}
          >
            Add credential
          </button>
        </div>
      </div>
    </div>
  );
}
