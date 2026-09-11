import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelInfo, ModelSchema } from '@litegen/sdk';
import { showToast } from './toast-store';

function buildCurl(model: ModelInfo): string {
  const endpoint =
    model.media_type === 'model3d' ? '/v1/models3d/generations'
    : model.media_type === 'video' ? '/v1/videos/generations'
    : '/v1/images/generations';
  const body = model.media_type === 'image'
    ? { model: model.id, prompt: 'a photo of a cat', n: 1 }
    : { model: model.id, prompt: 'a photo of a cat' };
  return [
    `curl -X POST $LITEGEN_BASE${endpoint} \\`,
    `  -H "Authorization: Bearer $LITEGEN_KEY" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '${JSON.stringify(body)}'`,
  ].join('\n');
}

/** Display name for every `ModelCapabilities` boolean flag, in the order the
 *  chips are rendered. Keep this in step with the schema: a flag with no entry
 *  here is invisible in the UI, and a model whose flags are ALL missing renders
 *  as '—', which reads as "no capabilities" rather than "not labelled yet". */
const CAP_LABELS: Record<string, string> = {
  supports_text_to_image: 'Text → image',
  supports_image_to_image: 'Image → image',
  supports_inpainting: 'Inpainting',
  supports_text_to_video: 'Text → video',
  supports_image_to_video: 'Image → video',
  supports_first_frame: 'First-frame ref',
  supports_last_frame: 'Last-frame ref',
  supports_text_to_3d: 'Text → 3D',
  supports_image_to_3d: 'Image → 3D',
  supports_multiview_to_3d: 'Multiview → 3D',
  supports_texture: 'Textures',
  supports_pbr: 'PBR materials',
  supports_rig: 'Auto-rigging',
};

/** Labels for the capability flags a model reports as true, CAP_LABELS order. */
// eslint-disable-next-line react-refresh/only-export-components -- pure helper, exported for its unit test
export function capabilityLabels(capabilities: unknown): string[] {
  const flags = (capabilities ?? {}) as Record<string, unknown>;
  return Object.entries(CAP_LABELS).filter(([k]) => flags[k]).map(([, label]) => label);
}

export default function ModelDetail({ model, onClose }: { model: ModelInfo; onClose: () => void }) {
  const [schema, setSchema] = useState<ModelSchema | null>(null);
  const [err, setErr] = useState('');

  useEffect(() => {
    let cancelled = false;
    setSchema(null); setErr('');
    client.models.getSchema(model.id)
      .then(s => { if (!cancelled) setSchema(s); })
      .catch(e => { if (!cancelled) setErr((e as Error).message); });
    return () => { cancelled = true; };
  }, [model.id]);

  const caps = capabilityLabels(model.capabilities);
  // ModelCapabilities.supported_sizes is string[] | undefined
  const sizes: string[] = (model.capabilities?.supported_sizes as string[] | undefined) ?? [];
  // A 3D model has no supported_sizes; output_formats is the analogous fact.
  const formats: string[] = (model.capabilities?.output_formats as string[] | undefined) ?? [];
  // ModelSchema.params is { [key: string]: unknown } | undefined; cast for describeSpec
  const params = (schema?.params ?? {}) as Record<string, { kind?: string; [k: string]: unknown }>;

  const copyCurl = async () => {
    try { await navigator.clipboard.writeText(buildCurl(model)); showToast('curl command copied'); }
    catch { showToast('Failed to copy', 'error'); }
  };

  const card: React.CSSProperties = { background: '#0d1117', border: '1px solid #30363d', borderRadius: 8, padding: 14, marginBottom: 16 };
  const h4: React.CSSProperties = { margin: '0 0 10px', fontSize: 13, color: '#8b949e', textTransform: 'uppercase', letterSpacing: '0.05em' };

  return (
    <div data-testid="model-detail-panel" style={{
      position: 'fixed', top: 0, right: 0, width: 480, height: '100vh', background: '#161b22',
      borderLeft: '1px solid #30363d', zIndex: 999, overflowY: 'auto', padding: 24,
    }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
        <h3 style={{ margin: 0, color: '#e6edf3' }}>{model.name ?? model.id}</h3>
        <button className="btn btn-secondary" data-testid="model-detail-close" onClick={onClose}>Close</button>
      </div>
      <p style={{ color: '#8b949e', fontFamily: 'monospace', fontSize: 13, marginBottom: 16 }}>{model.id}</p>

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
        <div style={h4}>Params &amp; allowed values</div>
        {err && <div className="alert alert-error">{err}</div>}
        {!schema && !err && <div style={{ color: '#8b949e', fontSize: 13 }}>Loading…</div>}
        {schema && Object.keys(params).length === 0 && <div style={{ color: '#6e7681', fontSize: 13 }}>No tunable params.</div>}
        {schema && Object.entries(params).map(([name, spec]) => (
          <div key={name} data-testid={`param-${name}`} style={{ display: 'flex', justifyContent: 'space-between', padding: '5px 0', borderBottom: '1px solid #21262d', fontSize: 13 }}>
            <span style={{ color: '#e6edf3', fontFamily: 'monospace' }}>{name}</span>
            <span style={{ color: '#8b949e' }}>{describeSpec(spec)}</span>
          </div>
        ))}
      </div>

      <button className="btn btn-secondary" data-testid="model-detail-copy-curl" onClick={copyCurl}>Copy as curl</button>
    </div>
  );
}

/** Compact one-line summary of a param spec (kind + allowed/range + default). */
function describeSpec(spec: { kind?: string; [k: string]: unknown }): string {
  const kind = spec.kind ?? 'value';
  const allowed = (spec.allowed as string[] | undefined) ?? (spec.enum_values as string[] | undefined);
  if (Array.isArray(allowed) && allowed.length) return `${kind}: ${allowed.join(' | ')}`;
  if (spec.min != null || spec.max != null) return `${kind} ${spec.min ?? ''}–${spec.max ?? ''}`;
  if (spec.default != null) return `${kind} (default ${String(spec.default)})`;
  return kind;
}
