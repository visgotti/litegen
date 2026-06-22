import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import { showToast } from './toast-store';

interface Props {
  activeApp: string | null | undefined;
  orgPool: Set<string>;
}

export default function AppModelAccessCard({ activeApp, orgPool }: Props) {
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

  const cardStyle: React.CSSProperties = {
    background: '#161b22', border: '1px solid #30363d', borderRadius: 10, padding: 20, marginBottom: 24,
  };
  const sectionTitle: React.CSSProperties = { margin: '0 0 16px', color: '#e6edf3', fontSize: 18, fontWeight: 600 };

  return (
    <div style={cardStyle}>
      <h3 style={sectionTitle}>App model access</h3>
      <p style={{ color: '#8b949e', fontSize: 13, margin: '0 0 16px' }}>
        Which org models this app&apos;s API keys may call.
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
          {[...orgPool].length === 0 ? (
            <div style={{ color: '#8b949e', fontSize: 14 }}>
              No models in org pool yet — add them above first.
            </div>
          ) : (
            [...orgPool].sort().map(modelId => (
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
  );
}
