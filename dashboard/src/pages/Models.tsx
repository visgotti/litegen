import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelInfo } from '@litegen/sdk';
import ModelDetail from '../components/ModelDetail';

export default function Models() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [error, setError] = useState('');
  const [selected, setSelected] = useState<ModelInfo | null>(null);

  // Filter state
  const [filterProvider, setFilterProvider] = useState('');
  const [filterMediaType, setFilterMediaType] = useState('');
  const [filterCapabilities, setFilterCapabilities] = useState<Set<string>>(new Set());

  useEffect(() => {
    client.models.list()
      .then(models => setModels(models))
      .catch(e => setError(e.message));
  }, []);

  // Derived values for filters
  const providers = Array.from(new Set(models.map(m => m.provider))).filter(Boolean);
  const allCapabilities = Array.from(
    new Set(models.flatMap(m => Object.keys(m.capabilities ?? {})))
  ).sort();

  const toggleCapability = (cap: string) => {
    setFilterCapabilities(prev => {
      const next = new Set(prev);
      if (next.has(cap)) next.delete(cap); else next.add(cap);
      return next;
    });
  };

  // Apply filters
  const filteredModels = models.filter(m => {
    if (filterProvider && m.provider !== filterProvider) return false;
    if (filterMediaType && m.media_type !== filterMediaType) return false;
    for (const cap of filterCapabilities) {
      if (!(cap in (m.capabilities ?? {}))) return false;
    }
    return true;
  });

  if (error) return <div className="alert alert-error">{error}</div>;
  if (!models.length) return <div className="loading">Loading models...</div>;

  return (
    <div style={{ position: 'relative' }}>
      <h2 className="page-title">Available Models</h2>

      {/* Filter row */}
      <div style={{
        background: '#161b22',
        border: '1px solid #30363d',
        borderRadius: 8,
        padding: 12,
        marginBottom: 16,
        display: 'flex',
        flexWrap: 'wrap',
        gap: 16,
        alignItems: 'flex-start',
      }}>
        {/* Provider dropdown */}
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Provider</label>
          <select
            className="input"
            data-testid="models-filter-provider"
            value={filterProvider}
            onChange={e => setFilterProvider(e.target.value)}
          >
            <option value="">All</option>
            {providers.map(p => (
              <option key={p} value={p}>{p}</option>
            ))}
          </select>
        </div>

        {/* Media type radio */}
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Media Type</label>
          <div
            data-testid="models-filter-media-type"
            style={{ display: 'flex', gap: 12, alignItems: 'center', paddingTop: 6 }}
          >
            {['', 'image', 'video'].map(val => (
              <label key={val} style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer', color: '#e1e4e8', fontSize: 14 }}>
                <input
                  type="radio"
                  name="media_type"
                  value={val}
                  checked={filterMediaType === val}
                  onChange={() => setFilterMediaType(val)}
                />
                {val === '' ? 'All' : val}
              </label>
            ))}
          </div>
        </div>

        {/* Capability checkboxes */}
        {allCapabilities.length > 0 && (
          <div>
            <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Capabilities</label>
            <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap', paddingTop: 4 }}>
              {allCapabilities.map(cap => (
                <label
                  key={cap}
                  style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer', color: '#e1e4e8', fontSize: 13 }}
                >
                  <input
                    type="checkbox"
                    data-testid={`models-filter-capability-${cap}`}
                    checked={filterCapabilities.has(cap)}
                    onChange={() => toggleCapability(cap)}
                  />
                  {cap}
                </label>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="table-container">
        <table>
          <thead>
            <tr>
              <th>Model ID</th>
              <th>Name</th>
              <th>Provider</th>
              <th>Type</th>
              <th>Cost</th>
              <th>Status</th>
              <th>Tags</th>
            </tr>
          </thead>
          <tbody>
            {filteredModels.map(m => (
              <tr
                key={m.id}
                data-testid={`model-row-${m.id}`}
                onClick={() => setSelected(m)}
                style={{ cursor: 'pointer' }}
                title="Click to view schema"
              >
                <td><code>{m.id}</code></td>
                <td>{m.name}</td>
                <td>{m.provider}</td>
                <td><span className={`badge ${m.media_type}`}>{m.media_type}</span></td>
                <td>{m.pricing ? `$${m.pricing.base_cost_usd.toFixed(4)}` : '—'}</td>
                <td>
                  <span className={`badge ${m.is_available ? 'healthy' : 'unhealthy'}`}>
                    {m.is_available ? 'available' : 'unavailable'}
                  </span>
                </td>
                <td>{(m.tags ?? []).join(', ')}</td>
              </tr>
            ))}
            {filteredModels.length === 0 && (
              <tr>
                <td colSpan={7} style={{ textAlign: 'center', color: '#8b949e' }}>No models match current filters</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {selected && <ModelDetail model={selected} onClose={() => setSelected(null)} />}
    </div>
  );
}
