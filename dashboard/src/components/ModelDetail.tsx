import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelInfo, ModelSchema } from '@litegen/sdk';
import { showToast } from './toast-store';
import { buildCurl, capabilityLabels, describePrompt, describeSpec, refRoleRows } from './model-detail';
import { docsUrlOf } from '../playground/params';

/** The schema fetch, tagged with the model it belongs to: the drawer derives
 *  "loading" from a mismatched id instead of resetting state inside an effect. */
interface Loaded {
  id: string;
  schema: ModelSchema | null;
  err: string;
}

export default function ModelDetail({ model, onClose }: { model: ModelInfo; onClose: () => void }) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.models.getSchema(model.id)
      .then(s => { if (!cancelled) setLoaded({ id: model.id, schema: s, err: '' }); })
      .catch(e => { if (!cancelled) setLoaded({ id: model.id, schema: null, err: (e as Error).message }); });
    return () => { cancelled = true; };
  }, [model.id]);

  const current = loaded?.id === model.id ? loaded : null;
  const schema = current?.schema ?? null;
  const err = current?.err ?? '';

  const caps = capabilityLabels(model.capabilities);
  // ModelCapabilities.supported_sizes is string[] | undefined
  const sizes: string[] = (model.capabilities?.supported_sizes as string[] | undefined) ?? [];
  // A 3D model has no supported_sizes; output_formats is the analogous fact.
  const formats: string[] = (model.capabilities?.output_formats as string[] | undefined) ?? [];
  // ModelSchema.params is { [key: string]: unknown } | undefined; cast for describeSpec
  const params = (schema?.params ?? {}) as Record<string, { kind?: string; [k: string]: unknown }>;
  const roles = refRoleRows(schema);
  const maxTotal = (schema as unknown as { ref_inputs?: { max_total?: number } } | null)?.ref_inputs?.max_total;
  const passthrough = (schema?.extra_allowlist ?? []) as string[];
  const docsUrl = docsUrlOf(schema ?? undefined);

  const copyCurl = async () => {
    try { await navigator.clipboard.writeText(buildCurl(model, schema)); showToast('curl command copied'); }
    catch { showToast('Failed to copy', 'error'); }
  };

  const card: React.CSSProperties = { background: '#0d1117', border: '1px solid #30363d', borderRadius: 8, padding: 14, marginBottom: 16 };
  const h4: React.CSSProperties = { margin: '0 0 10px', fontSize: 13, color: '#8b949e', textTransform: 'uppercase', letterSpacing: '0.05em' };
  const row: React.CSSProperties = { display: 'flex', justifyContent: 'space-between', gap: 12, padding: '5px 0', borderBottom: '1px solid #21262d', fontSize: 13 };

  return (
    <div data-testid="model-detail-panel" style={{
      position: 'fixed', top: 0, right: 0, width: 480, height: '100vh', background: '#161b22',
      borderLeft: '1px solid #30363d', zIndex: 999, overflowY: 'auto', padding: 24,
    }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
        <h3 style={{ margin: 0, color: '#e6edf3' }}>{model.name ?? model.id}</h3>
        <button className="btn btn-secondary" data-testid="model-detail-close" onClick={onClose}>Close</button>
      </div>
      <p style={{ color: '#8b949e', fontFamily: 'monospace', fontSize: 13, marginBottom: docsUrl ? 4 : 16 }}>{model.id}</p>
      {docsUrl && (
        <p style={{ margin: '0 0 16px', fontSize: 13 }}>
          <a href={docsUrl} target="_blank" rel="noreferrer" data-testid="model-detail-docs" style={{ color: 'var(--accent, #25b6ed)' }}>
            Provider API reference ↗
          </a>
        </p>
      )}

      <div style={card}>
        <div style={h4}>Capabilities</div>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
          {caps.map(label => (
            <span key={label} style={{ fontSize: 12, color: '#3fb950', background: '#3fb95022', padding: '3px 8px', borderRadius: 999 }}>{label}</span>
          ))}
          {caps.length === 0 && <span style={{ color: '#6e7681', fontSize: 13 }}>—</span>}
        </div>
        {sizes.length > 0 && (
          <div style={{ marginTop: 10, color: '#8b949e', fontSize: 13 }}>
            Sizes: <span style={{ color: '#e6edf3' }}>{sizes.join(', ')}</span>
          </div>
        )}
        {formats.length > 0 && (
          <div data-testid="model-detail-formats" style={{ marginTop: 10, color: '#8b949e', fontSize: 13 }}>
            Formats: <span style={{ color: '#e6edf3' }}>{formats.join(', ')}</span>
          </div>
        )}
      </div>

      <div style={card}>
        <div style={h4}>Pricing</div>
        <div style={{ color: '#e6edf3', fontSize: 14 }}>
          {model.pricing ? `$${model.pricing.base_cost_usd.toFixed(4)} base` : '—'}
          {model.pricing?.variable_pricing ? <span style={{ color: '#8b949e' }}> + variable</span> : null}
        </div>
      </div>

      <div style={card}>
        <div style={h4}>Inputs</div>
        {err && <div className="alert alert-error">{err}</div>}
        {!schema && !err && <div style={{ color: '#8b949e', fontSize: 13 }}>Loading…</div>}
        {schema && (
          <>
            <div data-testid="model-detail-prompt" style={row}>
              <span style={{ color: '#e6edf3', fontFamily: 'monospace' }}>prompt</span>
              <span style={{ color: '#8b949e', textAlign: 'right' }}>{describePrompt(schema)}</span>
            </div>
            {roles.map(r => (
              <div key={r.role} data-testid={`model-detail-ref-${r.role}`} style={row}>
                <span style={{ color: '#e6edf3' }}>
                  {r.label} <code style={{ color: '#8b949e', fontSize: 11 }}>{r.role}</code>
                </span>
                <span style={{ color: '#8b949e' }}>{r.rule}</span>
              </div>
            ))}
            {roles.length > 1 && maxTotal != null && (
              <div style={{ ...row, borderBottom: 'none' }}>
                <span style={{ color: '#8b949e' }}>Images per request</span>
                <span style={{ color: '#8b949e' }}>up to {maxTotal}</span>
              </div>
            )}
          </>
        )}
      </div>

      <div style={card}>
        <div style={h4}>Params &amp; allowed values</div>
        {!schema && !err && <div style={{ color: '#8b949e', fontSize: 13 }}>Loading…</div>}
        {schema && Object.keys(params).length === 0 && <div style={{ color: '#6e7681', fontSize: 13 }}>No tunable params.</div>}
        {schema && Object.entries(params).map(([name, spec]) => (
          <div key={name} data-testid={`param-${name}`} style={row}>
            <span style={{ color: '#e6edf3', fontFamily: 'monospace' }}>{name}</span>
            <span style={{ color: '#8b949e', textAlign: 'right' }}>{describeSpec(spec)}</span>
          </div>
        ))}
        {passthrough.length > 0 && (
          <div data-testid="model-detail-extra" style={{ marginTop: 10, color: '#8b949e', fontSize: 13 }}>
            Provider passthrough (<code>extra</code>):{' '}
            <span style={{ color: '#e6edf3', fontFamily: 'monospace' }}>{passthrough.join(', ')}</span>
          </div>
        )}
      </div>

      <button className="btn btn-secondary" data-testid="model-detail-copy-curl" onClick={copyCurl}>Copy as curl</button>
    </div>
  );
}
