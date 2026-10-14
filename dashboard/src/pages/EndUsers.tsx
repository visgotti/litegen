import { useCallback, useEffect, useState } from 'react';
import type { EndUserView, ModelInfo } from '@litegen/sdk';
import { client } from '../sdk-client';
import { useTenant } from '../context/tenant';
import { showToast } from '../components/toast-store';
import { orNull } from '../components/limits';
import LimitsEditor, { BudgetMeter } from '../components/LimitsEditor';
import '../management.css';

/** End users: your callers' customers, tracked per request with `x-litegen-end-user`. */
export default function EndUsers() {
  const { activeOrg } = useTenant();
  const [users, setUsers] = useState<EndUserView[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [newId, setNewId] = useState('');
  const [newAlias, setNewAlias] = useState('');
  const [newBudget, setNewBudget] = useState('');
  const [open, setOpen] = useState<string | null>(null);
  const [error, setError] = useState('');

  const load = useCallback(() => {
    client.endUsers.list()
      .then(u => { setUsers(u); setError(''); })
      .catch(e => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  useEffect(() => { load(); }, [load, activeOrg]);
  useEffect(() => { client.models.list().then(setModels).catch(() => setModels([])); }, []);

  const act = async (fn: () => Promise<unknown>, done: string) => {
    setError('');
    try {
      await fn();
      showToast(done, 'info');
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const create = () => {
    const id = newId.trim();
    if (!id) return;
    const budget = newBudget.trim() === '' ? undefined : Number(newBudget);
    if (budget !== undefined && (!Number.isFinite(budget) || budget < 0)) {
      setError('Budget must be a non-negative number');
      return;
    }
    void act(async () => {
      await client.endUsers.create({
        id,
        alias: newAlias.trim() || null,
        limits: budget === undefined ? null : { max_budget_usd: budget },
      });
      setNewId('');
      setNewAlias('');
      setNewBudget('');
    }, `End user ${id} created`);
  };

  return (
    <div>
      <h2 className="page-title">End users</h2>
      <p className="mg-hint">
        Send <code>x-litegen-end-user: &lt;id&gt;</code> with a generation to attribute it to one of your customers.
        Unknown ids are registered on first sight. Block an end user or give it a budget of its own.
      </p>
      {error && <div className="alert alert-error" data-testid="end-users-error">{error}</div>}

      <div className="mg-row mg-card">
        <label>End user id<input className="input" value={newId} placeholder="cust_123" data-testid="end-user-new-id" onChange={e => setNewId(e.target.value)} /></label>
        <label>Alias<input className="input" value={newAlias} placeholder="optional" data-testid="end-user-new-alias" onChange={e => setNewAlias(e.target.value)} /></label>
        <label>Budget (USD)<input className="input" value={newBudget} placeholder="unlimited" data-testid="end-user-new-budget" onChange={e => setNewBudget(e.target.value)} /></label>
        <button className="btn btn-primary" data-testid="end-user-create" disabled={!newId.trim()} onClick={create}>Add end user</button>
      </div>

      {users.length === 0 ? (
        <div className="mg-muted" data-testid="end-users-empty">No end users yet.</div>
      ) : (
        <div className="table-container">
          <table>
            <thead><tr><th>Id</th><th>Alias</th><th>Status</th><th>Budget</th><th>Last seen</th><th>Actions</th></tr></thead>
            <tbody>
              {users.map(u => (
                <tr key={u.id} data-testid={`end-user-row-${u.id}`}>
                  <td className="mg-mono">{u.id}</td>
                  <td>{u.alias ?? '—'}</td>
                  <td>
                    <span className={`badge ${u.blocked ? 'unhealthy' : 'healthy'}`} data-testid={`end-user-status-${u.id}`}>
                      {u.blocked ? 'blocked' : 'active'}
                    </span>
                  </td>
                  <td style={{ minWidth: 220 }}>{u.limits ? <BudgetMeter limits={u.limits} /> : <span className="mg-muted">none</span>}</td>
                  <td>{u.last_seen_at ? new Date(u.last_seen_at).toLocaleString() : 'never'}</td>
                  <td>
                    <div className="mg-actions" style={{ marginTop: 0 }}>
                      {u.blocked ? (
                        <button className="btn btn-secondary" data-testid={`end-user-unblock-${u.id}`}
                          onClick={() => void act(() => client.endUsers.unblock(u.id), `${u.id} unblocked`)}>Unblock</button>
                      ) : (
                        <button className="btn btn-secondary" data-testid={`end-user-block-${u.id}`}
                          onClick={() => void act(() => client.endUsers.block(u.id), `${u.id} blocked`)}>Block</button>
                      )}
                      <button className="btn btn-secondary" data-testid={`end-user-limits-${u.id}`}
                        onClick={() => setOpen(o => (o === u.id ? null : u.id))}>Limits</button>
                      <button className="btn btn-danger" data-testid={`end-user-delete-${u.id}`}
                        onClick={() => void act(() => client.endUsers.delete(u.id), `${u.id} deleted`)}>Delete</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {open && (
        <div style={{ marginTop: 16 }}>
          <LimitsEditor
            key={open}
            testid="end-user-limits"
            title={`End-user budget — ${open}`}
            models={models}
            load={() => orNull(client.endUsers.limits.get(open))}
            save={async b => { const r = await client.endUsers.limits.set(open, b); load(); return r; }}
            remove={async () => { await client.endUsers.limits.delete(open); load(); }}
            resetSpend={async () => { const r = await client.endUsers.limits.resetSpend(open); load(); return r; }}
          />
        </div>
      )}
    </div>
  );
}
