import { useEffect, useState } from 'react';
import { client, quietClient } from '../sdk-client';
import type { HealthResponse } from '@litegen/sdk';

type ProbeStatus = 'unknown' | 'alive' | 'ready' | 'not_ready' | 'error';

export default function Health() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // `/health` checks the gateway's own vendor keys, so only its operator reads
  // it (registry P1): anyone else is refused 403, and the page keeps the probes.
  const [operatorOnly, setOperatorOnly] = useState(false);
  const [error, setError] = useState('');
  const [liveness, setLiveness] = useState<ProbeStatus>('unknown');
  const [readiness, setReadiness] = useState<ProbeStatus>('unknown');
  // Bumped to fetch everything again (Refresh, and after the cache is cleared).
  const [fetches, setFetches] = useState(0);
  const refresh = () => setFetches(n => n + 1);

  useEffect(() => {
    let current = true;
    quietClient.health.get()
      .then(h => { if (current) { setHealth(h); setOperatorOnly(false); } })
      .catch(e => {
        if (!current) return;
        if ((e as { status?: unknown }).status === 403) setOperatorOnly(true);
        else setError(e.message);
      });
    // Liveness + readiness entirely through the SDK (the SDK throws on non-2xx).
    client.health.live()
      .then(() => 'alive' as const, () => 'error' as const)
      .then(s => { if (current) setLiveness(s); });
    client.health.ready()
      .then(() => 'ready' as const, () => 'not_ready' as const)
      .then(s => { if (current) setReadiness(s); });
    return () => { current = false; };
  }, [fetches]);

  const clearCache = async () => {
    try {
      await client.cache.clear();
      refresh();
    } catch (e: unknown) {
      setError((e as Error).message);
    }
  };

  if (error) return <div className="alert alert-error">{error}</div>;
  if (!health && !operatorOnly) return <div className="loading">Loading health data...</div>;

  const probes = (
    <>
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
    </>
  );

  if (!health) {
    return (
      <div>
        <div className="flex-between mb-24">
          <h2 className="page-title" style={{ margin: 0 }}>Provider Health</h2>
          <button className="btn btn-secondary" onClick={refresh}>Refresh</button>
        </div>
        <div className="stat-grid mb-24">{probes}</div>
        <p className="mg-hint" data-testid="health-provider-note">
          Provider health is the gateway operator&apos;s: it checks the gateway&apos;s own vendor keys, so only its
          operator reads it. Your organization&apos;s own provider keys are under Providers.
        </p>
      </div>
    );
  }

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
        {probes}
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
