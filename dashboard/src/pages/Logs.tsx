import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { client } from '../sdk-client';
import type { RequestLog, ModelInfo } from '@litegen/sdk';
import type { PaginatedResponse } from '@litegen/sdk';
import TracePanel from '../components/TracePanel';

const API_BASE = __LITEGEN_API_BASE__;

async function exportCsv(path: string, filename: string) {
  const apiKey = localStorage.getItem('litegen_api_key') ?? '';
  const authHeaders: Record<string, string> = apiKey
    ? { Authorization: `Bearer ${apiKey}` }
    : {};
  const res = await fetch(`${API_BASE}${path}&format=csv`, {
    headers: authHeaders,
    credentials: 'include',
  });
  if (!res.ok) return;
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

export default function Logs() {
  const [data, setData] = useState<PaginatedResponse<RequestLog> | null>(null);
  const [page, setPage] = useState(1);
  const [error, setError] = useState('');
  const [selectedLogId, setSelectedLogId] = useState<string | null>(null);

  // Filter state
  const [searchParams, setSearchParams] = useSearchParams();
  const [filterModel, setFilterModel] = useState(() => searchParams.get('model') ?? '');
  const [filterProvider, setFilterProvider] = useState(() => searchParams.get('provider') ?? '');
  const [filterStatus, setFilterStatus] = useState(() => searchParams.get('status') ?? '');
  const [filterFrom, setFilterFrom] = useState(() => searchParams.get('from') ?? '');
  const [filterTo, setFilterTo] = useState(() => searchParams.get('to') ?? '');

  // Models for dropdown
  const [models, setModels] = useState<ModelInfo[]>([]);

  const providers = Array.from(new Set(models.map(m => m.provider))).filter(Boolean);

  useEffect(() => {
    client.models.list()
      .then(models => setModels(models))
      .catch(() => {/* silently ignore */});
  }, []);

  const buildOpts = (p: number) => ({
    page: p,
    per_page: 25,
    model: searchParams.get('model') || undefined,
    provider: searchParams.get('provider') || undefined,
    status: searchParams.get('status') || undefined,
    from: searchParams.get('from') || undefined,
    to: searchParams.get('to') || undefined,
  });

  const load = (p: number) => {
    client.logs.list(buildOpts(p))
      .then(d => { setData(d); setPage(p); })
      .catch(e => setError(e.message));
  };

  // Re-fetch when URL params change
  useEffect(() => { load(1); }, [searchParams.toString()]);

  const applyFilters = () => {
    const params: Record<string, string> = {};
    if (filterModel) params.model = filterModel;
    if (filterProvider) params.provider = filterProvider;
    if (filterStatus) params.status = filterStatus;
    if (filterFrom) params.from = filterFrom;
    if (filterTo) params.to = filterTo;
    setSearchParams(params);
  };

  const clearFilters = () => {
    setFilterModel('');
    setFilterProvider('');
    setFilterStatus('');
    setFilterFrom('');
    setFilterTo('');
    setSearchParams({});
  };

  const handleExportCsv = () => {
    const params = new URLSearchParams();
    const model = searchParams.get('model');
    const provider = searchParams.get('provider');
    const status = searchParams.get('status');
    const from = searchParams.get('from');
    const to = searchParams.get('to');
    if (model) params.set('model', model);
    if (provider) params.set('provider', provider);
    if (status) params.set('status', status);
    if (from) params.set('from', from);
    if (to) params.set('to', to);
    const qs = params.toString();
    const today = new Date().toISOString().slice(0, 10).replace(/-/g, '');
    exportCsv(`/v1/logs?${qs}`, `logs-${today}.csv`);
  };

  if (error) return <div className="alert alert-error">{error}</div>;

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 }}>
        <h2 className="page-title" style={{ margin: 0 }}>Request Logs</h2>
        <button
          className="btn btn-secondary"
          data-testid="logs-export-csv"
          onClick={handleExportCsv}
        >
          Export CSV
        </button>
      </div>

      {/* Filter row */}
      <div style={{
        display: 'grid',
        gridTemplateColumns: '1fr 1fr 1fr 1fr 1fr auto auto',
        gap: 8,
        marginBottom: 16,
        background: '#161b22',
        border: '1px solid #30363d',
        borderRadius: 8,
        padding: 12,
        alignItems: 'end',
      }}>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Model</label>
          <select
            className="input"
            data-testid="logs-filter-model"
            value={filterModel}
            onChange={e => setFilterModel(e.target.value)}
            style={{ width: '100%' }}
          >
            <option value="">All</option>
            {models.map(m => (
              <option key={m.id} value={m.id}>{m.id}</option>
            ))}
          </select>
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Provider</label>
          <select
            className="input"
            data-testid="logs-filter-provider"
            value={filterProvider}
            onChange={e => setFilterProvider(e.target.value)}
            style={{ width: '100%' }}
          >
            <option value="">All</option>
            {providers.map(p => (
              <option key={p} value={p}>{p}</option>
            ))}
          </select>
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Status</label>
          <select
            className="input"
            data-testid="logs-filter-status"
            value={filterStatus}
            onChange={e => setFilterStatus(e.target.value)}
            style={{ width: '100%' }}
          >
            <option value="">All</option>
            <option value="completed">completed</option>
            <option value="failed">failed</option>
            <option value="pending">pending</option>
          </select>
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>From</label>
          <input
            type="date"
            className="input"
            data-testid="logs-filter-from"
            value={filterFrom}
            onChange={e => setFilterFrom(e.target.value)}
            style={{ width: '100%' }}
          />
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>To</label>
          <input
            type="date"
            className="input"
            data-testid="logs-filter-to"
            value={filterTo}
            onChange={e => setFilterTo(e.target.value)}
            style={{ width: '100%' }}
          />
        </div>
        <button
          className="btn btn-primary"
          data-testid="logs-filter-apply"
          onClick={applyFilters}
        >
          Apply
        </button>
        <button
          className="btn btn-secondary"
          data-testid="logs-filter-clear"
          onClick={clearFilters}
        >
          Clear
        </button>
      </div>

      {!data ? (
        <div className="loading">Loading logs...</div>
      ) : (
        <>
          <div className="table-container">
            <table>
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Model</th>
                  <th>Provider</th>
                  <th>Type</th>
                  <th>Status</th>
                  <th>Cost</th>
                  <th>Latency</th>
                  <th>Error</th>
                </tr>
              </thead>
              <tbody>
                {data.data.map(log => (
                  <tr
                    key={log.id}
                    data-testid={`logs-row-${log.id}`}
                    onClick={() => setSelectedLogId(log.id)}
                    style={{ cursor: 'pointer' }}
                  >
                    <td>{new Date(log.created_at).toLocaleString()}</td>
                    <td><code>{log.model}</code></td>
                    <td>{log.provider}</td>
                    <td><span className={`badge ${log.media_type}`}>{log.media_type}</span></td>
                    <td><span className={`badge ${log.status}`}>{log.status}</span></td>
                    <td>${log.cost_usd.toFixed(4)}</td>
                    <td>{log.latency_ms}ms</td>
                    <td style={{ maxWidth: 200, overflow: 'hidden', textOverflow: 'ellipsis' }}>{log.error || '—'}</td>
                  </tr>
                ))}
                {data.data.length === 0 && (
                  <tr><td colSpan={8} style={{ textAlign: 'center', color: '#8b949e' }}>No logs yet</td></tr>
                )}
              </tbody>
            </table>
          </div>
          {data.total_pages > 1 && (
            <div className="pagination">
              <button className="btn btn-secondary" disabled={page <= 1} onClick={() => load(page - 1)}>Previous</button>
              <span style={{ padding: '8px 12px', color: '#8b949e' }}>Page {page} of {data.total_pages}</span>
              <button className="btn btn-secondary" disabled={page >= data.total_pages} onClick={() => load(page + 1)}>Next</button>
            </div>
          )}
        </>
      )}

      <TracePanel
        logId={selectedLogId}
        onClose={() => setSelectedLogId(null)}
      />
    </div>
  );
}
