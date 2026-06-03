import { useEffect, useState } from 'react';
import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, PieChart, Pie, Cell } from 'recharts';
import { client } from '../sdk-client';
import type { ProxyStats, RequestLog, ApiKeyInfo } from '@litegen/sdk';
import { useAutoRefresh } from '../hooks/useAutoRefresh';
import { useTenant } from '../context/tenant';

const COLORS = ['#58a6ff', '#3fb950', '#d29922', '#f85149', '#bc8cff', '#f0883e'];

const AUTO_REFRESH_KEY = 'litegen_overview_autorefresh';

function buildHourlyBuckets(logs: RequestLog[]): { hour: string; cost: number }[] {
  const buckets = Array.from({ length: 24 }, (_, i) => ({ hour: String(i).padStart(2, '0') + ':00', cost: 0 }));
  for (const log of logs) {
    const d = new Date(log.created_at);
    const h = d.getHours();
    buckets[h].cost = parseFloat((buckets[h].cost + (log.cost_usd || 0)).toFixed(6));
  }
  return buckets;
}

export default function Overview() {
  const { activeOrg, activeApp } = useTenant();
  const [stats, setStats] = useState<ProxyStats | null>(null);
  const [error, setError] = useState('');
  const [logs, setLogs] = useState<RequestLog[]>([]);
  const [keys, setKeys] = useState<ApiKeyInfo[]>([]);

  const [autoRefresh, setAutoRefresh] = useState<boolean>(() => {
    return localStorage.getItem(AUTO_REFRESH_KEY) === 'true';
  });

  const fetchStats = () => {
    client.stats.get()
      .then(setStats)
      .catch(e => setError(e.message));
  };

  const fetchLogs = () => {
    client.logs.list({ page: 1, per_page: 500 })
      .then(r => setLogs(r.data))
      .catch(() => {/* silently ignore */});
  };

  const fetchKeys = () => {
    client.keys.list()
      .then(keys => setKeys(keys))
      .catch(() => {/* silently ignore */});
  };

  const fetchAll = () => {
    fetchStats();
    fetchLogs();
    fetchKeys();
  };

  // Refetch when the active tenant changes (SDK auto-scopes via headers).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { fetchAll(); }, [activeOrg, activeApp]);

  useAutoRefresh(fetchAll, 5000, autoRefresh);

  const toggleAutoRefresh = (val: boolean) => {
    setAutoRefresh(val);
    localStorage.setItem(AUTO_REFRESH_KEY, val ? 'true' : 'false');
  };

  if (error) return <div className="alert alert-error">{error}</div>;
  if (!stats) return <div className="loading">Loading stats...</div>;

  const successRate = stats.total_requests > 0
    ? ((stats.successful_requests / stats.total_requests) * 100).toFixed(1)
    : '0.0';

  const hourlyBuckets = buildHourlyBuckets(logs);
  const hasAnyTraffic = hourlyBuckets.some(b => b.cost > 0);

  const quotaKeys = keys.filter(k => k.is_active && k.token_quota != null);

  return (
    <div>
      {/* Page header with auto-refresh toggle */}
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
        <h2 className="page-title" style={{ margin: 0 }}>Overview</h2>
        <label
          data-testid="overview-autorefresh"
          style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', color: '#8b949e', fontSize: 14 }}
        >
          <input
            type="checkbox"
            checked={autoRefresh}
            onChange={e => toggleAutoRefresh(e.target.checked)}
            style={{ cursor: 'pointer' }}
          />
          Auto-refresh: 5s
        </label>
      </div>

      <div className="stat-grid">
        <div className="stat-card">
          <div className="label">Total Requests</div>
          <div className="value blue">{stats.total_requests.toLocaleString()}</div>
        </div>
        <div className="stat-card">
          <div className="label">Success Rate</div>
          <div className="value green">{successRate}%</div>
        </div>
        <div className="stat-card">
          <div className="label">Failed Requests</div>
          <div className="value red">{stats.failed_requests.toLocaleString()}</div>
        </div>
        <div className="stat-card">
          <div className="label">Total Cost</div>
          <div className="value">${stats.total_cost_usd.toFixed(4)}</div>
        </div>
        <div className="stat-card">
          <div className="label">Avg Latency</div>
          <div className="value">{stats.avg_latency_ms.toFixed(0)}ms</div>
        </div>
        <div className="stat-card">
          <div className="label">RPM</div>
          <div className="value blue">{stats.requests_per_minute.toFixed(1)}</div>
        </div>
        <div className="stat-card" data-testid="overview-p50">
          <div className="label">P50 Latency</div>
          <div className="value">{stats.latency_percentiles.p50_ms.toFixed(0)}ms</div>
        </div>
        <div className="stat-card" data-testid="overview-p95">
          <div className="label">P95 Latency</div>
          <div className="value">{stats.latency_percentiles.p95_ms.toFixed(0)}ms</div>
        </div>
        <div className="stat-card" data-testid="overview-p99">
          <div className="label">P99 Latency</div>
          <div className="value">{stats.latency_percentiles.p99_ms.toFixed(0)}ms</div>
        </div>
      </div>

      {stats.models_used.length > 0 && (
        <div className="chart-container">
          <h3 className="chart-title">Requests by Model</h3>
          <ResponsiveContainer width="100%" height={300}>
            <BarChart data={stats.models_used}>
              <CartesianGrid strokeDasharray="3 3" stroke="#30363d" />
              <XAxis dataKey="model" stroke="#8b949e" tick={{ fontSize: 12 }} />
              <YAxis stroke="#8b949e" />
              <Tooltip contentStyle={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 6, color: '#e1e4e8' }} />
              <Bar dataKey="requests" fill="#58a6ff" radius={[4, 4, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {stats.providers_used.length > 0 && (
        <div className="chart-container">
          <h3 className="chart-title">Cost by Provider</h3>
          <ResponsiveContainer width="100%" height={300}>
            <PieChart>
              <Pie
                data={stats.providers_used}
                dataKey="cost_usd"
                nameKey="provider"
                cx="50%"
                cy="50%"
                outerRadius={100}
                label={(props: any) => `${props.name ?? ''}: $${Number(props.value ?? 0).toFixed(4)}`}
              >
                {stats.providers_used.map((_, i) => (
                  <Cell key={i} fill={COLORS[i % COLORS.length]} />
                ))}
              </Pie>
              <Tooltip contentStyle={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 6, color: '#e1e4e8' }} />
            </PieChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* Cost over time chart */}
      <div className="chart-container" data-testid="overview-cost-chart">
        <h3 className="chart-title">Cost (last 24h, hourly)</h3>
        {hasAnyTraffic ? (
          <ResponsiveContainer width="100%" height={250}>
            <BarChart data={hourlyBuckets}>
              <CartesianGrid strokeDasharray="3 3" stroke="#30363d" />
              <XAxis dataKey="hour" stroke="#8b949e" tick={{ fontSize: 11 }} />
              <YAxis stroke="#8b949e" tickFormatter={(v: number) => `$${v.toFixed(4)}`} width={70} />
              <Tooltip
                contentStyle={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 6, color: '#e1e4e8' }}
                formatter={(value) => [`$${Number(value).toFixed(6)}`, 'Cost']}
              />
              <Bar dataKey="cost" fill="#58a6ff" radius={[4, 4, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        ) : (
          <p style={{ color: '#8b949e', textAlign: 'center', padding: '32px 0' }}>No traffic yet</p>
        )}
      </div>

      {/* Quota usage table */}
      <div className="chart-container" data-testid="overview-quota-table">
        <h3 className="chart-title">Quota Usage</h3>
        {quotaKeys.length === 0 ? (
          <p style={{ color: '#8b949e', textAlign: 'center', padding: '16px 0' }}>No keys with quota set</p>
        ) : (
          <table style={{ width: '100%', borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ borderBottom: '1px solid #30363d' }}>
                <th style={{ textAlign: 'left', padding: '8px 12px', color: '#8b949e', fontWeight: 600 }}>Name</th>
                <th style={{ textAlign: 'left', padding: '8px 12px', color: '#8b949e', fontWeight: 600 }}>Prefix</th>
                <th style={{ textAlign: 'left', padding: '8px 12px', color: '#8b949e', fontWeight: 600 }}>Used / Total</th>
                <th style={{ textAlign: 'left', padding: '8px 12px', color: '#8b949e', fontWeight: 600 }}>Remaining</th>
                <th style={{ textAlign: 'left', padding: '8px 12px', color: '#8b949e', fontWeight: 600 }}>RPM</th>
              </tr>
            </thead>
            <tbody>
              {quotaKeys.map(k => {
                const used = k.tokens_used ?? 0;
                const total = k.token_quota!;
                const remaining = Math.max(0, total - used);
                const pct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
                return (
                  <tr
                    key={k.id}
                    data-testid={`overview-quota-row-${k.id}`}
                    style={{ borderBottom: '1px solid #21262d' }}
                  >
                    <td style={{ padding: '10px 12px', color: '#e1e4e8' }}>{k.name}</td>
                    <td style={{ padding: '10px 12px' }}><code style={{ color: '#8b949e' }}>{k.prefix}...</code></td>
                    <td style={{ padding: '10px 12px', minWidth: 200 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                        <div style={{ flex: 1, background: '#21262d', borderRadius: 4, height: 8, overflow: 'hidden' }}>
                          <div style={{ width: `${pct}%`, background: pct > 80 ? '#f85149' : '#58a6ff', height: '100%', borderRadius: 4, transition: 'width 0.3s' }} />
                        </div>
                        <span style={{ color: '#8b949e', fontSize: 12, whiteSpace: 'nowrap' }}>${used.toFixed(2)} / ${total.toFixed(2)}</span>
                      </div>
                    </td>
                    <td style={{ padding: '10px 12px', color: '#3fb950' }}>${remaining.toFixed(2)}</td>
                    <td style={{ padding: '10px 12px', color: '#8b949e' }}>{k.rpm_limit ?? '—'}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
