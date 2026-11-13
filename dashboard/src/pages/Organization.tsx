import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { AppAccessSettings, AppStorageInfo, PutAppStorageRequest } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';
import { useCan } from '../components/useCan';
import { accessBody, describeAccess, type AccessForm } from '../components/access';
import { storageBody } from '../components/storage';

/** The access form for an app's stored settings (blank = use the gateway's). */
function accessFormOf(a: AppAccessSettings | null): AccessForm {
  const keep = a?.overrides.keep_outputs;
  return {
    mode: a?.overrides.mode ?? '',
    ownerScope: a?.overrides.owner_scope ?? '',
    ttl: a?.overrides.signed_url_ttl_seconds?.toString() ?? '',
    keepOutputs: keep === undefined || keep === null ? '' : String(keep),
    retentionDays: a?.overrides.retention_days?.toString() ?? '',
    signedLinks: a?.overrides.signed_links ?? '',
  };
}

export default function Organization() {
  const { orgs, activeOrg, activeApp, apps, refresh } = useTenant();
  // What this role may change (README roles: an app's settings, storage and
  // access are an Owner's or an Admin's); the rest is shown read-only.
  const can = useCan();
  const canRename = can('org:write');
  const canEditApps = can('app:write');
  const canDeleteApps = can('app:delete');
  const canEditStorage = can('storage_cred:write');

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

  // App storage (BYO S3)
  const [storage, setStorage] = useState<AppStorageInfo | null>(null);
  const [stBucket, setStBucket] = useState('');
  const [stRegion, setStRegion] = useState('us-east-1');
  const [stEndpoint, setStEndpoint] = useState('');
  const [stPublicUrl, setStPublicUrl] = useState('');
  const [stPrefix, setStPrefix] = useState('');
  const [stAccessKeyId, setStAccessKeyId] = useState('');
  const [stSecret, setStSecret] = useState('');
  const [stMode, setStMode] = useState<'access_key' | 'assume_role'>('access_key');
  const [stRoleArn, setStRoleArn] = useState('');
  const [stFallback, setStFallback] = useState(true);

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
      setStMode(s?.credential_mode === 'assume_role' ? 'assume_role' : 'access_key');
      setStRoleArn(s?.role_arn ?? '');
      setStFallback(s?.fallback_to_litegen_store ?? true);
    });
    return () => { cancelled = true; };
  }, [loadStorage]);

  const saveStorage = async () => {
    if (!activeApp) return;
    const { body, error } = storageBody({
      bucket: stBucket, region: stRegion, endpoint: stEndpoint, publicUrl: stPublicUrl, prefix: stPrefix,
      accessKeyId: stAccessKeyId, secret: stSecret, credentialMode: stMode, roleArn: stRoleArn, fallback: stFallback,
    });
    if (!body) {
      showToast(error ?? 'Invalid storage settings', 'error');
      return;
    }
    try {
      const updated = await client.apps.storage.put(activeApp, body as PutAppStorageRequest);
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

  // Who may open the app's stored files (docs/storage-access.md).
  const [access, setAccess] = useState<AppAccessSettings | null>(null);
  const [acForm, setAcForm] = useState<AccessForm>(() => accessFormOf(null));
  const [acError, setAcError] = useState('');
  // Another app (or none): nothing of the previous one's stays on screen while
  // the next one loads.
  const [accessApp, setAccessApp] = useState(activeApp);
  if (activeApp !== accessApp) {
    setAccessApp(activeApp);
    setAccess(null);
    setAcForm(accessFormOf(null));
    setAcError('');
  }

  useEffect(() => {
    if (!activeApp) return;
    let cancelled = false;
    client.apps.access.get(activeApp)
      .then(a => { if (!cancelled) { setAccess(a); setAcForm(accessFormOf(a)); } })
      .catch(() => { if (!cancelled) { setAccess(null); setAcForm(accessFormOf(null)); } });
    return () => { cancelled = true; };
  }, [activeApp]);

  const saveAccess = async () => {
    if (!activeApp) return;
    const { body, error } = accessBody(acForm);
    if (!body) {
      setAcError(error ?? 'Invalid settings');
      return;
    }
    setAcError('');
    try {
      const updated = await client.apps.access.put(activeApp, body);
      setAccess(updated);
      setAcForm(accessFormOf(updated));
      showToast('Access settings saved', 'info');
    } catch (err) {
      setAcError(err instanceof LiteGenAPIError ? (err.message ?? 'Save failed') : 'Save failed');
    }
  };

  const revokeLinks = async () => {
    if (!activeApp) return;
    try {
      await client.apps.access.revokeLinks(activeApp);
      setAccess(await client.apps.access.get(activeApp));
      showToast('Links revoked', 'info');
    } catch (err) {
      setAcError(err instanceof LiteGenAPIError ? (err.message ?? 'Revoke failed') : 'Revoke failed');
    }
  };

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

      {!(canRename && canEditApps && canDeleteApps && canEditStorage) && (
        <div data-testid="org-readonly-note" style={{ color: '#8b949e', fontSize: 13, margin: '-12px 0 20px' }}>
          Only an owner or an admin can change these settings; your role reads them.
        </div>
      )}

      {/* Rename org */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Name</h3>
        {canRename ? (
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
        ) : (
          <div data-testid="org-name" style={{ color: '#e6edf3', fontSize: 14 }}>{loadedOrgName}</div>
        )}
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
              {canDeleteApps && (
                <button
                  className="btn btn-danger"
                  data-testid={`app-delete-${a.id}`}
                  onClick={() => deleteApp(a.id)}
                  style={{ fontSize: 12, padding: '4px 10px' }}
                >
                  Delete
                </button>
              )}
            </div>
          ))}
        </div>
        {canEditApps && (
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
        )}
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
        {canEditStorage && (
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
            <label style={{ color: '#8b949e', fontSize: 13 }}>How litegen reaches the bucket
              <select className="input" data-testid="storage-credential-mode" value={stMode} disabled={!activeApp}
                onChange={e => setStMode(e.target.value === 'assume_role' ? 'assume_role' : 'access_key')}>
                <option value="access_key">An access key pair</option>
                <option value="assume_role">A cross-account IAM role (AWS)</option>
              </select>
            </label>
            {stMode === 'access_key' ? (
              <>
                <input className="input" data-testid="storage-access-key-id" value={stAccessKeyId}
                  onChange={e => setStAccessKeyId(e.target.value)}
                  placeholder={storage?.configured ? 'access key id (leave blank to keep)' : 'access key id'} disabled={!activeApp} />
                <input className="input" data-testid="storage-secret" type="password" value={stSecret}
                  onChange={e => setStSecret(e.target.value)}
                  placeholder={storage?.configured ? 'secret access key (leave blank to keep)' : 'secret access key'} disabled={!activeApp} />
              </>
            ) : (
              <>
                <input className="input" data-testid="storage-role-arn" value={stRoleArn}
                  onChange={e => setStRoleArn(e.target.value)} placeholder="role ARN (arn:aws:iam::123456789012:role/name)" disabled={!activeApp} />
                <div data-testid="storage-role-trust" style={{ color: '#8b949e', fontSize: 12, lineHeight: 1.6 }}>
                  The role's trust policy must trust{' '}
                  <code data-testid="storage-principal" style={{ color: '#e6edf3' }}>{storage?.litegen_principal_arn ?? "litegen's AWS account"}</code>
                  {' '}with the external ID{' '}
                  <code data-testid="storage-external-id" style={{ color: '#e6edf3' }}>{storage?.external_id ?? '(set a secrets key)'}</code>.
                  litegen assumes the role when you save.
                </div>
              </>
            )}
            <label style={{ color: '#8b949e', fontSize: 13, display: 'flex', gap: 8, alignItems: 'center' }}>
              <input type="checkbox" data-testid="storage-fallback" checked={stFallback} disabled={!activeApp}
                onChange={e => setStFallback(e.target.checked)} />
              While the bucket can't be reached, keep this app's files in litegen's own store
            </label>
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
        )}
      </div>

      {/* Who may open the app's stored files */}
      <div style={cardStyle} data-testid="access-card">
        <h3 style={sectionTitle}>Access to stored files</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          How the URLs of this app's images, videos and 3D files work, and who may open them.
          A field left on default follows the gateway's setting.
          {activeApp ? '' : ' — select an app to configure.'}
        </p>
        {access && (
          <div data-testid="access-effective" style={{ color: '#8b949e', fontSize: 13, marginBottom: 12 }}>
            Files follow: <strong style={{ color: '#e6edf3' }}>{describeAccess(access.effective)}</strong>
          </div>
        )}
        {canEditStorage && (
          <>
            {acError && <div className="alert alert-error" data-testid="access-error" style={{ marginBottom: 12 }}>{acError}</div>}
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 460 }}>
              <label style={{ color: '#8b949e', fontSize: 13 }}>Links
                <select className="input" data-testid="access-mode" value={acForm.mode} disabled={!activeApp}
                  onChange={e => setAcForm({ ...acForm, mode: e.target.value })}>
                  <option value="">Gateway default</option>
                  <option value="public">Public: the stored URL itself</option>
                  <option value="signed">Signed: links that expire</option>
                  <option value="private">Private: litegen's file route, with a key</option>
                </select>
              </label>
              <label style={{ color: '#8b949e', fontSize: 13 }}>Who may open a file through litegen
                <select className="input" data-testid="access-scope" value={acForm.ownerScope} disabled={!activeApp}
                  onChange={e => setAcForm({ ...acForm, ownerScope: e.target.value })}>
                  <option value="">Gateway default</option>
                  <option value="key">The key that made it</option>
                  <option value="app">Any key of this app</option>
                  <option value="org">Any key of the organization</option>
                </select>
              </label>
              <label style={{ color: '#8b949e', fontSize: 13 }}>Signed link life, in seconds
                <input className="input" data-testid="access-ttl" value={acForm.ttl} placeholder="gateway default (60 to 604800)"
                  disabled={!activeApp} onChange={e => setAcForm({ ...acForm, ttl: e.target.value })} />
              </label>
              <label style={{ color: '#8b949e', fontSize: 13 }}>Who signs a signed link
                <select className="input" data-testid="access-signed-links" value={acForm.signedLinks} disabled={!activeApp}
                  onChange={e => setAcForm({ ...acForm, signedLinks: e.target.value })}>
                  <option value="">Gateway default</option>
                  <option value="store">The store that holds the file (a bucket's link can't be revoked)</option>
                  <option value="litegen">litegen, for every file (revocable)</option>
                </select>
              </label>
              <label style={{ color: '#8b949e', fontSize: 13 }}>Keep outputs
                <select className="input" data-testid="access-keep" value={acForm.keepOutputs} disabled={!activeApp}
                  onChange={e => setAcForm({ ...acForm, keepOutputs: e.target.value })}>
                  <option value="">Gateway default</option>
                  <option value="true">Keep a copy of every output</option>
                  <option value="false">Keep nothing: images come back inline, video and 3D as the vendor's own links</option>
                </select>
              </label>
              <label style={{ color: '#8b949e', fontSize: 13 }}>Delete kept files after, in days
                <input className="input" data-testid="access-retention" value={acForm.retentionDays} placeholder="gateway default (0 keeps them until deleted)"
                  disabled={!activeApp} onChange={e => setAcForm({ ...acForm, retentionDays: e.target.value })} />
              </label>
              {access && access.effective.mode !== 'public' && (
                <p data-testid="access-bucket-note" style={{ color: '#e3b341', fontSize: 12, margin: 0 }}>
                  Signed and private links protect nothing while the bucket itself is public: keep the bucket
                  private (S3 Block Public Access).
                </p>
              )}
              <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
                <button className="btn btn-primary" data-testid="access-save" onClick={saveAccess} disabled={!activeApp}>
                  Save access
                </button>
                <button className="btn btn-danger" data-testid="access-revoke" onClick={revokeLinks} disabled={!activeApp}>
                  Revoke links
                </button>
                {access?.links_revoked_at && (
                  <span data-testid="access-revoked-at" style={{ color: '#8b949e', fontSize: 12 }}>
                    Links issued before {new Date(access.links_revoked_at).toLocaleString()} are refused.
                    A bucket's own links work until they expire.
                  </span>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
