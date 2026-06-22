import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { ProviderCatalogEntry, ModelInfo, ProviderCredentialInfo } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';
import ProviderSection from '../components/ProviderSection';
import ModelDetail from '../components/ModelDetail';
import AppModelAccessCard from '../components/AppModelAccessCard';

export default function Providers() {
  const { activeOrg, activeApp, activeOrgRole } = useTenant();
  const [catalog, setCatalog] = useState<ModelInfo[]>([]);
  const [providers, setProviders] = useState<ProviderCatalogEntry[]>([]);
  const [creds, setCreds] = useState<ProviderCredentialInfo[]>([]);
  const [enabled, setEnabled] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<ModelInfo | null>(null);

  useEffect(() => { void client.models.list().then(setCatalog).catch(() => {}); }, []);
  useEffect(() => { void client.providers.list().then(setProviders).catch(() => {}); }, []);

  const loadCreds = useCallback(async () => {
    if (!activeOrg) return;
    try { setCreds(await client.orgs.providerCredentials.list(activeOrg)); } catch { /* ignore */ }
  }, [activeOrg]);
  useEffect(() => { void loadCreds(); }, [loadCreds]);

  useEffect(() => {
    if (!activeOrg) return;
    let cancelled = false;
    void client.orgs.allowedModels.get(activeOrg).then(r => { if (!cancelled) setEnabled(new Set(r.models)); }).catch(() => {});
    return () => { cancelled = true; };
  }, [activeOrg]);

  // Auto-save a model toggle straight to the org pool.
  const toggleModel = async (id: string) => {
    if (!activeOrg) return;
    const next = new Set(enabled);
    if (next.has(id)) next.delete(id); else next.add(id);
    setEnabled(next); // optimistic
    try {
      const r = await client.orgs.allowedModels.set(activeOrg, { models: [...next] });
      setEnabled(new Set(r.models));
    } catch (err) {
      setEnabled(enabled); // revert
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error');
    }
  };

  const saveKey = (name: string) => async (poolField: string, entries: Record<string, unknown>[]) => {
    if (!activeOrg) return;
    try {
      await client.orgs.providerCredentials.create(activeOrg, { provider: name, credentials: { [poolField]: entries } });
      await loadCreds();
      showToast(`${name} key saved`, 'info');
    } catch (err) { if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Save failed', 'error'); }
  };
  const deleteKey = (name: string) => async () => {
    if (!activeOrg) return;
    try { await client.orgs.providerCredentials.delete(activeOrg, name); await loadCreds(); showToast(`${name} key removed`, 'info'); }
    catch (err) { if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error'); }
  };

  if (!activeOrg) return <div style={{ padding: 24, color: '#8b949e' }}>No active organization.</div>;
  if (activeOrgRole !== 'owner' && activeOrgRole !== 'admin')
    return <div style={{ padding: '24px 32px', background: '#3d1a1a', border: '1px solid #f85149', borderRadius: 8, color: '#f85149', margin: 24 }}>You need admin or owner access to manage providers.</div>;

  const byProvider: Record<string, ModelInfo[]> = {};
  for (const m of catalog) (byProvider[m.provider] ??= []).push(m);
  const credByProvider = new Map(creds.map(c => [c.provider, c]));
  const sorted = [...providers].sort((a, b) => (credByProvider.has(b.name) ? 1 : 0) - (credByProvider.has(a.name) ? 1 : 0) || a.name.localeCompare(b.name));

  return (
    <div style={{ padding: '0 0 32px' }}>
      <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Providers</h2>
      {sorted.map(p => (
        <ProviderSection
          key={p.name}
          provider={p}
          models={byProvider[p.name] ?? []}
          configured={credByProvider.has(p.name) || p.name === 'mock'}
          displayHint={credByProvider.get(p.name)?.display_hint ?? undefined}
          enabled={enabled}
          onToggleModel={toggleModel}
          onOpenModel={setDetail}
          onSaveKey={saveKey(p.name)}
          onDeleteKey={deleteKey(p.name)}
          defaultOpen={credByProvider.has(p.name)}
        />
      ))}
      <AppModelAccessCard activeApp={activeApp} orgPool={enabled} />
      {detail && <ModelDetail model={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}
