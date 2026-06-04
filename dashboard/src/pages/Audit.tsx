import React, { useEffect, useState, useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';
import { client } from '../sdk-client';
import type { AuditLogEntry } from '@litegen/sdk';

const API_BASE = __LITEGEN_API_BASE__;

async function exportCsv(path: string, filename: string) {
  const apiKey = localStorage.getItem('litegen_api_key') ?? '';
  const authHeaders: Record<string, string> = apiKey
    ? { Authorization: `Bearer ${apiKey}` }
    : {};
  const sep = path.includes('?') ? '&' : '?';
  const res = await fetch(`${API_BASE}${path}${sep}format=csv`, {
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

const ACTION_OPTIONS = [
  '',
  'key.create',
  'key.update',
  'key.revoke',
  'key.rotate',
  'key.test_webhook',
  'generation.cancel',
];

export default function Audit() {
  const [searchParams, setSearchParams] = useSearchParams();

  const pageParam = parseInt(searchParams.get('page') ?? '1', 10);
  const actionParam = searchParams.get('action') ?? '';
  const actorParam = searchParams.get('actor_key_id') ?? '';
  const fromParam = searchParams.get('from') ?? '';
  const toParam = searchParams.get('to') ?? '';

  // Filter form state (not yet applied)
  const [filterActor, setFilterActor] = useState(actorParam);
  const [filterAction, setFilterAction] = useState(actionParam);
  const [filterFrom, setFilterFrom] = useState(fromParam);
  const [filterTo, setFilterTo] = useState(toParam);

  const [entries, setEntries] = useState<AuditLogEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [totalPages, setTotalPages] = useState(1);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  // Expanded rows map: id -> open
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const load = useCallback(() => {
    setLoading(true);
    setError('');
    client.audit.list({
      page: pageParam,
      per_page: 50,
      actor_key_id: actorParam || undefined,
      action: actionParam || undefined,
      from: fromParam || undefined,
      to: toParam || undefined,
    })
      .then(r => {
        setEntries(r.data);
        setTotal(r.total);
        setTotalPages(r.total_pages);
      })
      .catch(e => setError(e.message))
      .finally(() => setLoading(false));
  }, [pageParam, actorParam, actionParam, fromParam, toParam]);

  useEffect(() => { load(); }, [load]);

  const applyFilters = () => {
    const p: Record<string, string> = { page: '1' };
    if (filterActor.trim()) p['actor_key_id'] = filterActor.trim();
    if (filterAction) p['action'] = filterAction;
    if (filterFrom.trim()) p['from'] = filterFrom.trim();
    if (filterTo.trim()) p['to'] = filterTo.trim();
    setSearchParams(p);
  };

  const clearFilters = () => {
    setFilterActor('');
    setFilterAction('');
    setFilterFrom('');
    setFilterTo('');
    setSearchParams({ page: '1' });
  };

  const toggleExpand = (id: string) => {
    setExpanded(prev => ({ ...prev, [id]: !prev[id] }));
  };

  const truncate = (s: string | null, len = 80) => {
    if (!s) return '—';
    return s.length > len ? s.slice(0, len) + '…' : s;
  };

  const handleExportCsv = () => {
    const params = new URLSearchParams();
    if (actorParam) params.set('actor_key_id', actorParam);
    if (actionParam) params.set('action', actionParam);
    if (fromParam) params.set('from', fromParam);
    if (toParam) params.set('to', toParam);
    const qs = params.toString();
    const today = new Date().toISOString().slice(0, 10).replace(/-/g, '');
    exportCsv(`/v1/audit${qs ? '?' + qs : ''}`, `audit-${today}.csv`);
  };

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 }}>
        <h2 className="page-title" style={{ margin: 0 }}>Audit Log</h2>
        <button
          className="btn btn-secondary"
          data-testid="audit-export-csv"
          onClick={handleExportCsv}
        >
          Export CSV
        </button>
      </div>

      {error && <div className="alert alert-error">{error}</div>}

      {/* Filter row */}
      <div
        style={{
          background: '#161b22',
          border: '1px solid #30363d',
          borderRadius: 8,
          padding: 12,
          marginBottom: 16,
          display: 'flex',
          gap: 8,
          flexWrap: 'wrap',
          alignItems: 'flex-end',
        }}
      >
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Actor Key ID</label>
          <input
            className="input"
            data-testid="audit-filter-actor"
            placeholder="key-uuid"
            value={filterActor}
            onChange={e => setFilterActor(e.target.value)}
          />
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Action</label>
          <select
            className="input"
            data-testid="audit-filter-action"
            value={filterAction}
            onChange={e => setFilterAction(e.target.value)}
            style={{ minWidth: 160 }}
          >
            {ACTION_OPTIONS.map(a => (
              <option key={a} value={a}>{a || '— all —'}</option>
            ))}
          </select>
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>From</label>
          <input
            className="input"
            data-testid="audit-filter-from"
            type="datetime-local"
            value={filterFrom}
            onChange={e => setFilterFrom(e.target.value)}
          />
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>To</label>
          <input
            className="input"
            data-testid="audit-filter-to"
            type="datetime-local"
            value={filterTo}
            onChange={e => setFilterTo(e.target.value)}
          />
        </div>
        <button
          className="btn btn-primary"
          data-testid="audit-filter-apply"
          onClick={applyFilters}
        >
          Apply
        </button>
        <button
          className="btn btn-secondary"
          data-testid="audit-filter-clear"
          onClick={clearFilters}
        >
          Clear
        </button>
      </div>

      {loading && <div className="loading">Loading audit log…</div>}

      <div className="table-container">
        <table data-testid="audit-table">
          <thead>
            <tr>
              <th>Timestamp</th>
              <th>Actor</th>
              <th>Action</th>
              <th>Target Type</th>
              <th>Target ID</th>
              <th>Before</th>
              <th>After</th>
            </tr>
          </thead>
          <tbody>
            {entries.map(e => (
              <React.Fragment key={e.id}>
                <tr
                  data-testid={`audit-row-${e.id}`}
                  onClick={() => toggleExpand(e.id)}
                  style={{ cursor: 'pointer' }}
                >
                  <td style={{ whiteSpace: 'nowrap' }}>
                    {new Date(e.created_at).toLocaleString()}
                  </td>
                  <td>{e.actor_label}</td>
                  <td><code>{e.action}</code></td>
                  <td>{e.target_type}</td>
                  <td><code style={{ fontSize: 11 }}>{e.target_id.slice(0, 12)}…</code></td>
                  <td style={{ fontSize: 11, color: '#8b949e' }}>{truncate(e.before_json)}</td>
                  <td style={{ fontSize: 11, color: '#8b949e' }}>{truncate(e.after_json)}</td>
                </tr>
                {expanded[e.id] && (
                  <tr key={`detail-${e.id}`} data-testid={`audit-detail-${e.id}`}>
                    <td colSpan={7}>
                      <div
                        style={{
                          background: '#0d1117',
                          border: '1px solid #30363d',
                          borderRadius: 6,
                          padding: 12,
                          fontSize: 12,
                          fontFamily: 'monospace',
                        }}
                      >
                        <div style={{ marginBottom: 8 }}>
                          <strong style={{ color: '#8b949e' }}>Full ID:</strong>{' '}
                          <span>{e.id}</span>
                        </div>
                        <div style={{ marginBottom: 8 }}>
                          <strong style={{ color: '#8b949e' }}>Actor Key ID:</strong>{' '}
                          <span>{e.actor_key_id ?? 'master-key'}</span>
                        </div>
                        {e.before_json && (
                          <div style={{ marginBottom: 8 }}>
                            <strong style={{ color: '#8b949e' }}>Before:</strong>
                            <pre style={{ margin: '4px 0 0 0', overflowX: 'auto', color: '#e6edf3' }}>
                              {JSON.stringify(JSON.parse(e.before_json), null, 2)}
                            </pre>
                          </div>
                        )}
                        {e.after_json && (
                          <div>
                            <strong style={{ color: '#8b949e' }}>After:</strong>
                            <pre style={{ margin: '4px 0 0 0', overflowX: 'auto', color: '#e6edf3' }}>
                              {JSON.stringify(JSON.parse(e.after_json), null, 2)}
                            </pre>
                          </div>
                        )}
                      </div>
                    </td>
                  </tr>
                )}
              </React.Fragment>
            ))}
            {!loading && entries.length === 0 && (
              <tr>
                <td colSpan={7} style={{ textAlign: 'center', color: '#8b949e' }}>
                  No audit log entries
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Pagination */}
      {totalPages > 1 && (
        <div style={{ display: 'flex', gap: 8, marginTop: 12, alignItems: 'center' }}>
          <button
            className="btn btn-secondary"
            disabled={pageParam <= 1}
            onClick={() => setSearchParams({ ...Object.fromEntries(searchParams), page: String(pageParam - 1) })}
          >
            Previous
          </button>
          <span style={{ color: '#8b949e', fontSize: 13 }}>
            Page {pageParam} / {totalPages} ({total} entries)
          </span>
          <button
            className="btn btn-secondary"
            disabled={pageParam >= totalPages}
            onClick={() => setSearchParams({ ...Object.fromEntries(searchParams), page: String(pageParam + 1) })}
          >
            Next
          </button>
        </div>
      )}
    </div>
  );
}
