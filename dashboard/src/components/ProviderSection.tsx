import { useState } from 'react';
import type { ProviderCatalogEntry, ModelInfo } from '@litegen/sdk';

type CredRow = { values: Record<string, string>; weight: string };
const emptyRow = (): CredRow => ({ values: {}, weight: '1' });

interface Props {
  provider: ProviderCatalogEntry;
  models: ModelInfo[];
  configured: boolean;
  displayHint?: string;
  enabled: Set<string>;
  onToggleModel: (id: string) => void;
  onOpenModel: (m: ModelInfo) => void;
  onSaveKey: (poolField: string, entries: Record<string, unknown>[]) => Promise<void>;
  onDeleteKey: () => Promise<void>;
  defaultOpen: boolean;
}

export default function ProviderSection(props: Props) {
  const { provider, models, configured, displayHint, enabled, onToggleModel, onOpenModel, onSaveKey, onDeleteKey, defaultOpen } = props;
  const [open, setOpen] = useState(defaultOpen);
  const [rows, setRows] = useState<CredRow[]>([emptyRow()]);

  const setVal = (i: number, k: string, v: string) =>
    setRows(rs => rs.map((r, idx) => idx === i ? { ...r, values: { ...r.values, [k]: v } } : r));
  const setW = (i: number, v: string) => setRows(rs => rs.map((r, idx) => idx === i ? { ...r, weight: v } : r));
  const addRow = () => setRows(rs => [...rs, emptyRow()]);
  const removeRow = (i: number) => setRows(rs => rs.length > 1 ? rs.filter((_, idx) => idx !== i) : rs);

  const save = async () => {
    const entries = rows.map(row => {
      const obj: Record<string, unknown> = {};
      for (const f of provider.fields) {
        const v = (row.values[f.key] ?? '').trim();
        if (v) obj[f.key] = v;
      }
      obj.weight = Math.max(1, parseInt(row.weight, 10) || 1);
      return obj;
    }).filter(obj => provider.fields.every(f => f.optional || typeof obj[f.key] === 'string'));
    if (entries.length === 0) return;
    await onSaveKey(provider.pool_field, entries);
    setRows([emptyRow()]);
  };

  const enabledCount = models.filter(m => enabled.has(m.id)).length;

  return (
    <div data-testid={`provider-section-${provider.name}`} style={{ border: '1px solid #30363d', borderRadius: 10, marginBottom: 12, background: '#161b22' }}>
      <button
        data-testid={`provider-toggle-${provider.name}`}
        onClick={() => setOpen(o => !o)}
        style={{ display: 'flex', width: '100%', alignItems: 'center', gap: 10, padding: '14px 16px', background: 'none', border: 'none', cursor: 'pointer', color: '#e6edf3' }}
      >
        <span style={{ color: configured ? '#3fb950' : '#6e7681' }}>{configured ? '●' : '○'}</span>
        <strong style={{ fontSize: 15 }}>{provider.name}</strong>
        <span style={{ color: '#6e7681', fontSize: 12 }}>{configured ? 'configured' : 'needs key'}</span>
        <span style={{ marginLeft: 'auto', color: '#8b949e', fontSize: 12 }}>{enabledCount}/{models.length} enabled</span>
        <span style={{ color: '#8b949e' }}>{open ? '▾' : '▸'}</span>
      </button>

      {open && (
        <div style={{ padding: '0 16px 16px' }}>
          {/* Key block (org-scoped) */}
          <div style={{ background: '#0d1117', border: '1px solid #30363d', borderRadius: 8, padding: 12, marginBottom: 14 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
              <span style={{ color: '#8b949e', fontSize: 12, textTransform: 'uppercase', letterSpacing: '0.05em' }}>API key (org-wide)</span>
              {configured && displayHint && (
                <span style={{ color: '#8b949e', fontFamily: 'monospace', fontSize: 13 }}>
                  {displayHint}
                  <button className="btn btn-danger" data-testid={`provider-key-delete-${provider.name}`} onClick={onDeleteKey} style={{ marginLeft: 10, fontSize: 12, padding: '2px 8px' }}>Delete</button>
                </span>
              )}
            </div>
            {rows.map((row, i) => (
              <div key={i} style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 6 }}>
                {provider.fields.map(f => (
                  <input key={f.key} className="input"
                    data-testid={`provider-key-field-${provider.name}-${f.key}-${i}`}
                    type={f.secret ? 'password' : 'text'}
                    value={row.values[f.key] ?? ''} onChange={e => setVal(i, f.key, e.target.value)}
                    placeholder={f.optional ? `${f.label} (optional)` : f.label} style={{ maxWidth: 220 }} />
                ))}
                <input className="input" type="number" min={1} value={row.weight}
                  onChange={e => setW(i, e.target.value)} title="Weight" style={{ width: 80 }} />
                <button className="btn btn-danger" onClick={() => removeRow(i)} disabled={rows.length <= 1} style={{ fontSize: 12, padding: '4px 10px' }}>×</button>
              </div>
            ))}
            <div style={{ display: 'flex', gap: 8 }}>
              <button className="btn" onClick={addRow} style={{ fontSize: 13 }}>+ Add another key</button>
              <button className="btn btn-primary" data-testid={`provider-key-save-${provider.name}`} onClick={save}>Save key</button>
            </div>
          </div>

          {/* Models block (org-scoped enable, auto-save) */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
            {models.map(m => (
              <div key={m.id} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 8px', borderRadius: 6 }}>
                <input type="checkbox" data-testid={`provider-model-enable-${m.id}`} checked={enabled.has(m.id)} onChange={() => onToggleModel(m.id)} />
                <span style={{ color: '#e6edf3', fontSize: 14, flex: 1 }}>{m.name ?? m.id}</span>
                <span className={`badge ${m.media_type}`}>{m.media_type}</span>
                <span style={{ color: '#8b949e', fontSize: 12, width: 64, textAlign: 'right' }}>{m.pricing ? `$${m.pricing.base_cost_usd.toFixed(4)}` : '—'}</span>
                <button className="btn btn-secondary" data-testid={`provider-model-open-${m.id}`} onClick={() => onOpenModel(m)} style={{ fontSize: 12, padding: '2px 8px' }}>›</button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
