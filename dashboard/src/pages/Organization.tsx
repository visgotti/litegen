import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { ProviderCredentialInfo, AppStorageInfo, PutAppStorageRequest } from '@litegen/sdk';
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

  // App storage (BYO S3)
  const [storage, setStorage] = useState<AppStorageInfo | null>(null);
  const [stBucket, setStBucket] = useState('');
  const [stRegion, setStRegion] = useState('us-east-1');
  const [stEndpoint, setStEndpoint] = useState('');
  const [stPublicUrl, setStPublicUrl] = useState('');
  const [stPrefix, setStPrefix] = useState('');
  const [stAccessKeyId, setStAccessKeyId] = useState('');
  const [stSecret, setStSecret] = useState('');

  const loadStorage = useCallback(async () => {
    if (!activeApp) return null;
    try {
      return await client.apps.storage.get(activeApp);
    } catch {
      return null;
    }
  }, [activeApp]);

  useEffect(() => {
    let cancelled = false;
    void loadStorage().then(s => {
      if (cancelled) return;
      setStorage(s);
      setStBucket(s?.bucket_name ?? '');
      setStRegion(s?.region ?? 'us-east-1');
      setStEndpoint(s?.endpoint_url ?? '');
      setStPublicUrl(s?.custom_public_url ?? '');
      setStPrefix(s?.path_prefix ?? '');
      setStAccessKeyId('');
      setStSecret('');
    });
    return () => { cancelled = true; };
  }, [loadStorage]);

  const saveStorage = async () => {
    if (!activeApp || !stBucket.trim()) return;
    try {
      const body: PutAppStorageRequest = {
        bucket_name: stBucket.trim(),
        region: stRegion.trim() || 'us-east-1',
        endpoint_url: stEndpoint.trim() || undefined,
        custom_public_url: stPublicUrl.trim() || undefined,
        path_prefix: stPrefix.trim() || undefined,
      };
      if (stAccessKeyId.trim() && stSecret.trim()) {
        body.access_key_id = stAccessKeyId.trim();
        body.secret_access_key = stSecret.trim();
      }
      const updated = await client.apps.storage.put(activeApp, body);
      setStorage(updated);
      setStAccessKeyId('');
      setStSecret('');
      showToast('Storage configuration saved', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const removeStorage = async () => {
    if (!activeApp) return;
    try {
      await client.apps.storage.delete(activeApp);
      setStorage(await loadStorage());
      setStBucket(''); setStEndpoint(''); setStPublicUrl(''); setStPrefix('');
      setStAccessKeyId(''); setStSecret('');
      showToast('Storage configuration removed', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error');
    }
  };

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

      {/* App storage (BYO S3) */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Storage (BYO S3)</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Generated images upload to this app's own bucket. Leave unset to use the platform default.
          {activeApp ? '' : ' — select an app to configure.'}
        </p>
        {storage?.configured && (
          <div data-testid="storage-configured" style={{ color: '#8b949e', fontSize: 13, marginBottom: 12 }}>
            Configured · bucket <strong style={{ color: '#e6edf3' }}>{storage.bucket_name}</strong>
            {storage.access_key_id_hint && (
              <span style={{ fontFamily: 'monospace', marginLeft: 8 }}>key {storage.access_key_id_hint}</span>
            )}
          </div>
        )}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 460 }}>
          <input className="input" data-testid="storage-bucket" value={stBucket}
            onChange={e => setStBucket(e.target.value)} placeholder="bucket name" disabled={!activeApp} />
          <input className="input" data-testid="storage-region" value={stRegion}
            onChange={e => setStRegion(e.target.value)} placeholder="region (e.g. us-east-1)" disabled={!activeApp} />
          <input className="input" data-testid="storage-endpoint" value={stEndpoint}
            onChange={e => setStEndpoint(e.target.value)} placeholder="endpoint URL (MinIO/R2/Spaces, optional)" disabled={!activeApp} />
          <input className="input" data-testid="storage-public-url" value={stPublicUrl}
            onChange={e => setStPublicUrl(e.target.value)} placeholder="custom public URL / CDN (optional)" disabled={!activeApp} />
          <input className="input" data-testid="storage-prefix" value={stPrefix}
            onChange={e => setStPrefix(e.target.value)} placeholder="path prefix (default litegen/images)" disabled={!activeApp} />
          <input className="input" data-testid="storage-access-key-id" value={stAccessKeyId}
            onChange={e => setStAccessKeyId(e.target.value)}
            placeholder={storage?.configured ? 'access key id (leave blank to keep)' : 'access key id'} disabled={!activeApp} />
          <input className="input" data-testid="storage-secret" type="password" value={stSecret}
            onChange={e => setStSecret(e.target.value)}
            placeholder={storage?.configured ? 'secret access key (leave blank to keep)' : 'secret access key'} disabled={!activeApp} />
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn btn-primary" data-testid="storage-save" onClick={saveStorage} disabled={!activeApp}>
              Save storage
            </button>
            {storage?.configured && (
              <button className="btn btn-danger" data-testid="storage-remove" onClick={removeStorage} disabled={!activeApp}>
                Remove storage
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
