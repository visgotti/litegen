import { useCallback, useEffect, useMemo, useState } from 'react';
import type { ApiKeyInfo, SpendGroupBy, SpendLogsResponse, SpendReportResponse } from '@litegen/sdk';
import { client } from '../sdk-client';
import { useTenant } from '../context/tenant';
import { GROUP_BY_OPTIONS, formatUsd, isoDay, spendBars } from '../components/limits';
import '../management.css';

const PER_PAGE = 25;

/** Spend reports grouped by any dimension, and the attributed request log. */
export default function Spend() {
  const { activeOrg, activeApp, apps } = useTenant();
  const [from, setFrom] = useState(isoDay(30));
  const [to, setTo] = useState(isoDay(0));
  const [groupBy, setGroupBy] = useState<SpendGroupBy>('day');
  const [endUser, setEndUser] = useState('');
  const [tag, setTag] = useState('');
  const [applied, setApplied] = useState({ from: isoDay(30), to: isoDay(0), endUser: '', tag: '' });
  const [report, setReport] = useState<SpendReportResponse | null>(null);
  const [logs, setLogs] = useState<SpendLogsResponse | null>(null);
  const [page, setPage] = useState(1);
  const [keys, setKeys] = useState<ApiKeyInfo[]>([]);
  const [error, setError] = useState('');

  useEffect(() => {
    client.keys.list().then(setKeys).catch(() => setKeys([]));
  }, [activeOrg, activeApp]);

  const filters = useMemo(() => ({
    from: applied.from || undefined,
    to: applied.to || undefined,
    endUser: applied.endUser || undefined,
    tag: applied.tag || undefined,
  }), [applied]);

  const load = useCallback(() => {
    client.spend.report({ ...filters, groupBy })
      .then(r => { setReport(r); setError(''); })
      .catch(e => setError(e instanceof Error ? e.message : String(e)));
    client.spend.logs({ ...filters, page, perPage: PER_PAGE })
      .then(setLogs)
      .catch(e => setError(e instanceof Error ? e.message : String(e)));
  }, [filters, groupBy, page]);

  useEffect(() => { load(); }, [load, activeOrg, activeApp]);

  const names = useMemo(() => {
    const n: Record<string, string> = {};
    keys.forEach(k => { n[k.id] = `${k.name} (${k.prefix}…)`; });
    apps.forEach(a => { n[a.id] = a.name; });
    return n;
  }, [keys, apps]);

  const bars = report ? spendBars(report.data, names) : [];
  const totalPages = logs ? Math.max(1, Math.ceil(logs.total / PER_PAGE)) : 1;

  return (
    <div>
      <h2 className="page-title">Spend</h2>
      <p className="mg-hint">
        Every request is attributed to its key, user, end user (<code>x-litegen-end-user</code>) and tags
        (<code>x-litegen-tags</code>). Group spend by any of them.
      </p>
      {error && <div className="alert alert-error" data-testid="spend-error">{error}</div>}

      <div className="mg-row mg-card">
        <label>From<input className="input" type="date" value={from} data-testid="spend-from" onChange={e => setFrom(e.target.value)} /></label>
        <label>To<input className="input" type="date" value={to} data-testid="spend-to" onChange={e => setTo(e.target.value)} /></label>
        <label>End user<input className="input" placeholder="any" value={endUser} data-testid="spend-end-user" onChange={e => setEndUser(e.target.value)} /></label>
        <label>Tag<input className="input" placeholder="any" value={tag} data-testid="spend-tag" onChange={e => setTag(e.target.value)} /></label>
        <button className="btn btn-primary" data-testid="spend-apply"
          onClick={() => { setPage(1); setApplied({ from, to, endUser: endUser.trim(), tag: tag.trim() }); }}>
          Apply
        </button>
      </div>

      <div className="stat-grid">
        <div className="stat-card">
          <div className="label">Total spend</div>
          <div className="value blue" data-testid="spend-total">{formatUsd(report?.total_cost_usd ?? 0)}</div>
        </div>
        <div className="stat-card">
          <div className="label">Requests</div>
          <div className="value" data-testid="spend-requests">{report?.total_requests ?? 0}</div>
        </div>
      </div>

      <div className="mg-card">
        <div className="mg-card-head">
          <h3>Spend by</h3>
          <select className="input" value={groupBy} data-testid="spend-group-by" onChange={e => setGroupBy(e.target.value as SpendGroupBy)}>
            {GROUP_BY_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
          </select>
        </div>
        {bars.length === 0 ? (
          <div className="mg-muted" data-testid="spend-report-empty">No spend in this range.</div>
        ) : (
          <div className="table-container">
            <table data-testid="spend-report">
              <thead><tr><th>{GROUP_BY_OPTIONS.find(o => o.value === groupBy)?.label}</th><th>Requests</th><th>Failed</th><th>Cost</th><th /></tr></thead>
              <tbody>
                {bars.map(b => (
                  <tr key={b.label} data-testid="spend-report-row">
                    <td className="mg-mono">{b.label}</td>
                    <td>{b.requests}</td>
                    <td>{b.failed}</td>
                    <td>{formatUsd(b.cost)}</td>
                    <td className="mg-bar-cell"><div className="mg-bar" style={{ width: `${b.width}%` }} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <div className="mg-card">
        <div className="mg-card-head"><h3>Spend log</h3><span className="mg-muted" data-testid="spend-log-total">{logs?.total ?? 0} requests</span></div>
        <div className="table-container">
          <table data-testid="spend-log">
            <thead><tr><th>Time</th><th>Model</th><th>Status</th><th>Cost</th><th>Key</th><th>End user</th><th>Tags</th></tr></thead>
            <tbody>
              {(logs?.data ?? []).map(l => (
                <tr key={l.id} data-testid="spend-log-row">
                  <td>{new Date(l.created_at).toLocaleString()}</td>
                  <td className="mg-mono">{l.model}</td>
                  <td><span className={`badge ${l.status}`}>{l.status}</span></td>
                  <td>{formatUsd(l.cost_usd)}</td>
                  <td className="mg-mono">{l.key_id ? names[l.key_id] ?? l.key_id.slice(0, 8) : '—'}</td>
                  <td className="mg-mono">{l.end_user ?? '—'}</td>
                  <td>{l.tags.join(', ') || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="pagination">
          <button className="btn btn-secondary" disabled={page <= 1} onClick={() => setPage(p => p - 1)}>Previous</button>
          <span className="mg-muted">Page {page} of {totalPages}</span>
          <button className="btn btn-secondary" disabled={page >= totalPages} onClick={() => setPage(p => p + 1)}>Next</button>
        </div>
      </div>
    </div>
  );
}
