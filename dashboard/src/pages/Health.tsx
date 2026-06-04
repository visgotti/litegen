import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { HealthResponse } from '@litegen/sdk';

const BASE = __LITEGEN_API_BASE__;

type ProbeStatus = 'unknown' | 'alive' | 'ready' | 'not_ready' | 'error';

export default function Health() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [error, setError] = useState('');
  const [liveness, setLiveness] = useState<ProbeStatus>('unknown');
  const [readiness, setReadiness] = useState<ProbeStatus>('unknown');

  const refreshProbes = async () => {
    // Liveness
    try {
      await client.health.live();
      setLiveness('alive');
    } catch {
      // Fall back to raw fetch for non-JSON or network error
      try {
        const r = await fetch(`${BASE}/health/live`, { credentials: 'include' });
        setLiveness(r.ok ? 'alive' : 'error');
      } catch {
        setLiveness('error');
      }
    }
    // Readiness
    try {
      await client.health.ready();
      setReadiness('ready');
    } catch {
      try {
        const r = await fetch(`${BASE}/health/ready`, { credentials: 'include' });
        setReadiness(r.ok ? 'ready' : 'not_ready');
      } catch {
        setReadiness('not_ready');
      }
    }
  };

  const refresh = () => {
    client.health.get()
      .then(setHealth)
      .catch(e => setError(e.message));
    refreshProbes();
  };

  useEffect(() => { refresh(); }, []);

  const clearCache = async () => {
    try {
      await client.cache.clear();
      refresh();
    } catch (e: unknown) {
      setError((e as Error).message);
    }
  };

  if (error) return <div className="alert alert-error">{error}</div>;
  if (!health) return <div className="loading">Loading health data...</div>;

  return (
    <div>
      <div className="flex-between mb-24">
        <h2 className="page-title" style={{ margin: 0 }}>Provider Health</h2>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-secondary" onClick={refresh}>Refresh</button>
          <button className="btn btn-danger" onClick={clearCache}>Clear Cache</button>
        </div>
      </div>

      <div className="stat-grid mb-24">
        <div className="stat-card">
          <div className="label">Liveness</div>
          <div
            className={`value ${liveness === 'alive' ? 'green' : liveness === 'unknown' ? '' : 'red'}`}
            data-testid="health-liveness-badge"
          >
            {liveness === 'alive' ? '✓ alive' : liveness === 'unknown' ? '…' : '✗ error'}
          </div>
        </div>
        <div className="stat-card">
          <div className="label">Readiness</div>
          <div
            className={`value ${readiness === 'ready' ? 'green' : readiness === 'unknown' ? '' : 'red'}`}
            data-testid="health-readiness-badge"
          >
            {readiness === 'ready' ? '✓ ready' : readiness === 'unknown' ? '…' : '✗ not ready'}
          </div>
        </div>
        <div className="stat-card">
          <div className="label">Overall Status</div>
          <div className={`value ${health.status === 'healthy' ? 'green' : 'red'}`}>
            {health.status}
          </div>
        </div>
        <div className="stat-card">
          <div className="label">Cache</div>
          <div className="value blue">
            {health.cache.enabled ? `${health.cache.entries} entries` : 'Disabled'}
          </div>
        </div>
      </div>

      <div className="table-container">
        <table>
          <thead>
            <tr>
              <th>Provider</th>
              <th>Status</th>
              <th>Message</th>
              <th>Latency</th>
              <th>Last Checked</th>
            </tr>
          </thead>
          <tbody>
            {health.providers.map((p, idx) => (
              <tr key={`${p.provider}-${idx}`}>
                <td><strong>{p.provider}</strong></td>
                <td>
                  <span className={`badge ${p.healthy ? 'healthy' : 'unhealthy'}`}>
                    {p.healthy ? 'healthy' : 'unhealthy'}
                  </span>
                </td>
                <td>{p.message || '—'}</td>
                <td>{p.latency_ms ? `${p.latency_ms}ms` : '—'}</td>
                <td>{p.last_checked ? new Date(p.last_checked).toLocaleTimeString() : '—'}</td>
              </tr>
            ))}
            {health.providers.length === 0 && (
              <tr><td colSpan={5} style={{ textAlign: 'center', color: '#8b949e' }}>No providers configured</td></tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
