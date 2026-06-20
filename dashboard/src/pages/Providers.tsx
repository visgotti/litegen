import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type {
  ProviderCredentialInfo,
  ProviderCatalogEntry,
  ModelInfo,
} from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';

export default function Providers() {
  const { activeOrg, activeApp, activeOrgRole } = useTenant();

  // ── Org model pool ────────────────────────────────────────────────────────
  const [catalog, setCatalog] = useState<ModelInfo[]>([]);
  const [orgModels, setOrgModels] = useState<Set<string>>(new Set());
  const [orgModelsDirty, setOrgModelsDirty] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void client.models.list().then(models => {
      if (cancelled) return;
      setCatalog(models);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (!activeOrg) return;
    let cancelled = false;
    void client.orgs.allowedModels.get(activeOrg).then(r => {
      if (cancelled) return;
      setOrgModels(new Set(r.models));
      setOrgModelsDirty(false);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [activeOrg]);

  const toggleOrgModel = (modelId: string) => {
    setOrgModels(prev => {
      const next = new Set(prev);
      if (next.has(modelId)) next.delete(modelId); else next.add(modelId);
      return next;
    });
    setOrgModelsDirty(true);
  };

  const saveOrgModels = async () => {
    if (!activeOrg) return;
    try {
      const r = await client.orgs.allowedModels.set(activeOrg, { models: [...orgModels] });
      setOrgModels(new Set(r.models));
      setOrgModelsDirty(false);
      showToast('Org model pool saved', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  // Group catalog by provider prefix (e.g. "openai/dall-e-3" → "openai")
  const byProvider: Record<string, ModelInfo[]> = {};
  for (const m of catalog) {
    const provider = m.id.split('/')[0] ?? m.id;
    (byProvider[provider] ??= []).push(m);
  }

  // ── App credentials ───────────────────────────────────────────────────────
  const [creds, setCreds] = useState<ProviderCredentialInfo[]>([]);
  const [providerCatalog, setProviderCatalog] = useState<ProviderCatalogEntry[]>([]);
  const [credProvider, setCredProvider] = useState('');
  type CredRow = { values: Record<string, string>; weight: string };
  const emptyRow = (): CredRow => ({ values: {}, weight: '1' });
  const [credRows, setCredRows] = useState<CredRow[]>([emptyRow()]);

  const loadCreds = useCallback(async () => {
    if (!activeApp) return [] as ProviderCredentialInfo[];
    try { return await client.apps.providerCredentials.list(activeApp); }
    catch { return [] as ProviderCredentialInfo[]; }
  }, [activeApp]);

  useEffect(() => {
    let cancelled = false;
    void loadCreds().then(list => { if (!cancelled) setCreds(list); });
    return () => { cancelled = true; };
  }, [loadCreds]);

  useEffect(() => {
    let cancelled = false;
    void client.providers.list()
      .then(list => {
        if (cancelled) return;
        setProviderCatalog(list);
        setCredProvider(prev => prev || list[0]?.name || '');
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);

  const selectedProvider = providerCatalog.find(p => p.name === credProvider) ?? null;
  const changeProvider = (name: string) => { setCredProvider(name); setCredRows([emptyRow()]); };
  const setRowValue = (i: number, key: string, value: string) =>
    setCredRows(rows => rows.map((r, idx) => idx === i ? { ...r, values: { ...r.values, [key]: value } } : r));
  const setRowWeight = (i: number, value: string) =>
    setCredRows(rows => rows.map((r, idx) => idx === i ? { ...r, weight: value } : r));
  const addRow = () => setCredRows(rows => [...rows, emptyRow()]);
  const removeRow = (i: number) =>
    setCredRows(rows => rows.length > 1 ? rows.filter((_, idx) => idx !== i) : rows);

  const addCred = async () => {
    if (!activeApp || !selectedProvider) return;
    const provider = selectedProvider;
    const entries = credRows
      .map(row => {
        const obj: Record<string, unknown> = {};
        for (const f of provider.fields) {
          const v = (row.values[f.key] ?? '').trim();
          if (v) obj[f.key] = v;
        }
        obj.weight = Math.max(1, parseInt(row.weight, 10) || 1);
        return obj;
      })
      .filter(obj => provider.fields.every(f => f.optional || typeof obj[f.key] === 'string'));
    if (entries.length === 0) { showToast('Fill in at least one credential', 'error'); return; }
    try {
      await client.apps.providerCredentials.create(activeApp, {
        provider: provider.name,
        credentials: { [provider.pool_field]: entries },
      });
      setCredRows([emptyRow()]);
      setCreds(await loadCreds());
      showToast(`Provider ${entries.length === 1 ? 'credential' : `credentials (${entries.length})`} saved`, 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
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

  // ── App model access ──────────────────────────────────────────────────────
  const [appModelMode, setAppModelMode] = useState<'all' | 'select'>('all');
  const [appModelIds, setAppModelIds] = useState<Set<string>>(new Set());
  const [appModelDirty, setAppModelDirty] = useState(false);

  useEffect(() => {
    if (!activeApp) return;
    let cancelled = false;
    void client.apps.allowedModels.get(activeApp).then(r => {
      if (cancelled) return;
      setAppModelMode(r.mode as 'all' | 'select');
      setAppModelIds(new Set(r.models));
      setAppModelDirty(false);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [activeApp]);

  const saveAppModels = async () => {
    if (!activeApp) return;
    try {
      const r = await client.apps.allowedModels.set(activeApp, {
        mode: appModelMode,
        models: appModelMode === 'select' ? [...appModelIds] : [],
      });
      setAppModelMode(r.mode as 'all' | 'select');
      setAppModelIds(new Set(r.models));
      setAppModelDirty(false);
      showToast('App model access saved', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const toggleAppModel = (modelId: string) => {
    setAppModelIds(prev => {
      const next = new Set(prev);
      if (next.has(modelId)) next.delete(modelId); else next.add(modelId);
      return next;
    });
    setAppModelDirty(true);
  };

  // ── Permission gate ───────────────────────────────────────────────────────
  if (!activeOrg) {
    return (
      <div style={{ padding: 24, color: '#8b949e' }}>
        No active organization.
      </div>
    );
  }

  if (activeOrgRole !== 'owner' && activeOrgRole !== 'admin') {
    return (
      <div style={{
        padding: '24px 32px', background: '#3d1a1a', border: '1px solid #f85149',
        borderRadius: 8, color: '#f85149', margin: 24,
      }}>
        You need admin or owner access to manage providers.
      </div>
    );
  }

  const cardStyle: React.CSSProperties = {
    background: '#161b22', border: '1px solid #30363d', borderRadius: 10, padding: 20, marginBottom: 24,
  };
  const sectionTitle: React.CSSProperties = { margin: '0 0 16px', color: '#e6edf3', fontSize: 18, fontWeight: 600 };
  const badge = (text: string, color: string) => (
    <span style={{ fontSize: 11, fontWeight: 600, padding: '2px 7px', borderRadius: 999, background: color + '22', color, marginLeft: 6 }}>
      {text}
    </span>
  );

  // ── Render ────────────────────────────────────────────────────────────────
  return (
    <div style={{ padding: '0 0 32px' }}>
      <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Providers</h2>

      {/* Card 1 — Org model pool */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>Org model pool</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Models enabled for this org. Apps may only use models from this pool.
          {orgModels.size === 0 && (
            <span style={{ marginLeft: 8, color: '#e3b341', fontWeight: 600 }}>
              ⚠ No models enabled — apps have no access.
            </span>
          )}
        </p>
        {catalog.length === 0 ? (
          <div style={{ color: '#8b949e', fontSize: 14 }}>Loading model catalog…</div>
        ) : (
          Object.entries(byProvider).map(([provider, models]) => (
            <div key={provider} style={{ marginBottom: 16 }}>
              <div style={{ color: '#8b949e', fontSize: 12, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>
                {provider}
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                {models.map(m => (
                  <label key={m.id} style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', padding: '6px 10px', borderRadius: 6, background: orgModels.has(m.id) ? '#1a3f5c22' : 'transparent' }}>
                    <input
                      type="checkbox"
                      checked={orgModels.has(m.id)}
                      onChange={() => toggleOrgModel(m.id)}
                    />
                    <span style={{ color: '#e6edf3', fontSize: 14 }}>{m.name ?? m.id}</span>
                    {badge(m.media_type, m.media_type === 'video' ? '#d2a8ff' : '#3fb950')}
                  </label>
                ))}
              </div>
            </div>
          ))
        )}
        <button
          className="btn btn-primary"
          onClick={saveOrgModels}
          disabled={!orgModelsDirty}
          style={{ marginTop: 8 }}
        >
          Save org model pool
        </button>
      </div>

      {/* Card 2 — App credentials */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>App credentials</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Provider API keys scoped to the active app{activeApp ? '' : ' — select an app to manage credentials'}.
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
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
            <label style={{ color: '#8b949e', fontSize: 13 }}>Provider</label>
            <select
              className="input"
              data-testid="provider-cred-provider"
              value={credProvider}
              onChange={e => changeProvider(e.target.value)}
              disabled={!activeApp || providerCatalog.length === 0}
              style={{ maxWidth: 220 }}
            >
              {providerCatalog.map(p => (
                <option key={p.name} value={p.name}>{p.name}</option>
              ))}
            </select>
            {selectedProvider && selectedProvider.modalities.length > 0 && (
              <span style={{ color: '#6e7681', fontSize: 12 }}>
                {selectedProvider.modalities.join(' + ')}
              </span>
            )}
          </div>
          {selectedProvider && (
            <>
              {credRows.map((row, i) => (
                <div
                  key={i}
                  data-testid={`provider-cred-input-row-${i}`}
                  style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}
                >
                  {selectedProvider.fields.map(f => (
                    <input
                      key={f.key}
                      className="input"
                      data-testid={`provider-cred-field-${f.key}-${i}`}
                      type={f.secret ? 'password' : 'text'}
                      value={row.values[f.key] ?? ''}
                      onChange={e => setRowValue(i, f.key, e.target.value)}
                      placeholder={f.optional ? `${f.label} (optional)` : f.label}
                      style={{ maxWidth: 220 }}
                      disabled={!activeApp}
                    />
                  ))}
                  <input
                    className="input"
                    data-testid={`provider-cred-weight-${i}`}
                    type="number" min={1}
                    value={row.weight}
                    onChange={e => setRowWeight(i, e.target.value)}
                    title="Weight (higher = more traffic)"
                    style={{ width: 84 }}
                    disabled={!activeApp}
                  />
                  <button
                    className="btn btn-danger"
                    data-testid={`provider-cred-remove-row-${i}`}
                    onClick={() => removeRow(i)}
                    disabled={!activeApp || credRows.length <= 1}
                    style={{ fontSize: 12, padding: '4px 10px' }}
                    title="Remove this key"
                  >×</button>
                </div>
              ))}
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <button className="btn" data-testid="provider-cred-add-row" onClick={addRow} disabled={!activeApp} style={{ fontSize: 13 }}>
                  + Add another {selectedProvider.pool_field === 'credential_sets' ? 'credential' : 'key'}
                </button>
                <button className="btn btn-primary" data-testid="provider-cred-add" onClick={addCred} disabled={!activeApp}>
                  Save credential
                </button>
              </div>
              <p style={{ color: '#6e7681', fontSize: 12, margin: 0 }}>
                {credRows.length > 1
                  ? 'Requests are load-balanced across these by weight (higher = more traffic).'
                  : 'Add more than one to load-balance requests across them by weight.'}
              </p>
            </>
          )}
        </div>
      </div>

      {/* Card 3 — App model access */}
      <div style={cardStyle}>
        <h3 style={sectionTitle}>App model access</h3>
        <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
          Which org models this app's API keys may call.
          {activeApp ? '' : ' — select an app to configure.'}
        </p>
        <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
          {(['all', 'select'] as const).map(m => (
            <button
              key={m}
              className={`btn${appModelMode === m ? ' btn-primary' : ''}`}
              onClick={() => { setAppModelMode(m); setAppModelDirty(true); }}
              disabled={!activeApp}
              style={{ minWidth: 100 }}
            >
              {m === 'all' ? 'All org models' : 'Select'}
            </button>
          ))}
        </div>
        {appModelMode === 'select' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginBottom: 16 }}>
            {[...orgModels].length === 0 ? (
              <div style={{ color: '#8b949e', fontSize: 14 }}>
                No models in org pool yet — add them above first.
              </div>
            ) : (
              [...orgModels].sort().map(modelId => (
                <label key={modelId} style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', padding: '6px 10px', borderRadius: 6 }}>
                  <input
                    type="checkbox"
                    checked={appModelIds.has(modelId)}
                    onChange={() => toggleAppModel(modelId)}
                    disabled={!activeApp}
                  />
                  <span style={{ color: '#e6edf3', fontSize: 14 }}>{modelId}</span>
                </label>
              ))
            )}
          </div>
        )}
        <button
          className="btn btn-primary"
          onClick={saveAppModels}
          disabled={!activeApp || !appModelDirty}
        >
          Save app model access
        </button>
      </div>
    </div>
  );
}
