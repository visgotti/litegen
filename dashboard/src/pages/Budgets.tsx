import { useEffect, useMemo, useState } from 'react';
import type { ApiKeyInfo, EntityLimits, MemberView, ModelInfo } from '@litegen/sdk';
import { client } from '../sdk-client';
import { useTenant } from '../context/tenant';
import LimitsEditor, { BudgetMeter } from '../components/LimitsEditor';
import { entityLabel, orNull } from '../components/limits';
import '../management.css';

const TYPE_LABEL: Record<string, string> = {
  org: 'Organization',
  app: 'Application',
  member: 'Member',
  key: 'API key',
  end_user: 'End user',
};

/** Budgets and limits for the active org, its app, its members and its keys. */
export default function Budgets() {
  const { activeOrg, activeApp, apps } = useTenant();
  const [rows, setRows] = useState<EntityLimits[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [members, setMembers] = useState<MemberView[]>([]);
  const [keys, setKeys] = useState<ApiKeyInfo[]>([]);
  const [memberId, setMemberId] = useState('');
  const [keyId, setKeyId] = useState('');
  const [error, setError] = useState('');
  const [version, setVersion] = useState(0);

  useEffect(() => {
    client.models.list().then(setModels).catch(() => setModels([]));
    client.keys.list().then(k => { setKeys(k); setKeyId(id => id || k[0]?.id || ''); }).catch(() => setKeys([]));
  }, [activeOrg, activeApp]);

  useEffect(() => {
    if (!activeOrg) return;
    client.orgs.members.list(activeOrg)
      .then(m => { setMembers(m); setMemberId(id => id || m[0]?.user_id || ''); })
      .catch(() => setMembers([]));
  }, [activeOrg]);

  useEffect(() => {
    client.limits.list()
      .then(r => { setRows(r); setError(''); })
      .catch(e => setError(e instanceof Error ? e.message : String(e)));
  }, [activeOrg, activeApp, version]);

  const names = useMemo(() => {
    const n: Record<string, string> = {};
    members.forEach(m => { n[m.user_id] = m.email; });
    keys.forEach(k => { n[k.id] = `${k.name} (${k.prefix}…)`; });
    apps.forEach(a => { n[a.id] = a.name; });
    return n;
  }, [members, keys, apps]);

  const refresh = () => setVersion(v => v + 1);
  const wrap = <T,>(p: Promise<T>) => p.then(r => { refresh(); return r; });

  return (
    <div>
      <h2 className="page-title">Budgets &amp; limits</h2>
      <p className="mg-hint">
        Spend caps with reset windows, soft-budget alerts, model allowlists and rate limits. Every request is checked
        against its key, member, application and organization — the strictest limit wins.
      </p>
      {error && <div className="alert alert-error">{error}</div>}

      <div className="mg-card" data-testid="limits-overview">
        <div className="mg-card-head"><h3>All limits in this organization</h3></div>
        {rows.length === 0 ? (
          <div className="mg-muted" data-testid="limits-empty">No limits set yet.</div>
        ) : (
          <div className="table-container">
            <table>
              <thead>
                <tr><th>Type</th><th>Who</th><th>Budget</th><th>RPM</th><th>Parallel</th><th>Models</th></tr>
              </thead>
              <tbody>
                {rows.map(r => (
                  <tr key={`${r.entity_type}:${r.entity_id}`} data-testid={`limits-row-${r.entity_type}`}>
                    <td>{TYPE_LABEL[r.entity_type] ?? r.entity_type}</td>
                    <td className="mg-mono">{entityLabel(r, names)}</td>
                    <td style={{ minWidth: 240 }}><BudgetMeter limits={r} /></td>
                    <td>{r.rpm_limit ?? '—'}</td>
                    <td>{r.max_parallel_requests ?? '—'}</td>
                    <td>{r.allowed_models ? r.allowed_models.length : 'all'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {!activeOrg ? (
        <div className="alert alert-error">Sign in with an organization session to manage org, app and member limits.</div>
      ) : (
        <>
          <LimitsEditor
            testid="org-limits"
            title="Organization budget"
            models={models}
            load={() => orNull(client.orgs.limits.get(activeOrg))}
            save={b => wrap(client.orgs.limits.set(activeOrg, b))}
            remove={() => wrap(client.orgs.limits.delete(activeOrg))}
            resetSpend={() => wrap(client.orgs.limits.resetSpend(activeOrg))}
          />
          {activeApp && (
            <LimitsEditor
              key={activeApp}
              testid="app-limits"
              title={`Application budget — ${names[activeApp] ?? activeApp}`}
              models={models}
              load={() => orNull(client.apps.limits.get(activeApp))}
              save={b => wrap(client.apps.limits.set(activeApp, b))}
              remove={() => wrap(client.apps.limits.delete(activeApp))}
              resetSpend={() => wrap(client.apps.limits.resetSpend(activeApp))}
            />
          )}
          <div className="mg-row">
            <label>
              Member
              <select className="input" value={memberId} data-testid="member-select" onChange={e => setMemberId(e.target.value)}>
                {members.map(m => <option key={m.user_id} value={m.user_id}>{m.email} ({m.role})</option>)}
              </select>
            </label>
          </div>
          {memberId && (
            <LimitsEditor
              key={memberId}
              testid="member-limits"
              title={`Member budget — ${names[memberId] ?? memberId}`}
              models={models}
              load={() => orNull(client.orgs.memberLimits.get(activeOrg, memberId))}
              save={b => wrap(client.orgs.memberLimits.set(activeOrg, memberId, b))}
              remove={() => wrap(client.orgs.memberLimits.delete(activeOrg, memberId))}
              resetSpend={() => wrap(client.orgs.memberLimits.resetSpend(activeOrg, memberId))}
            />
          )}
        </>
      )}

      <div className="mg-row">
        <label>
          API key
          <select className="input" value={keyId} data-testid="key-select" onChange={e => setKeyId(e.target.value)}>
            {keys.map(k => <option key={k.id} value={k.id}>{k.name} ({k.prefix}…)</option>)}
          </select>
        </label>
      </div>
      {keyId && (
        <LimitsEditor
          key={keyId}
          testid="key-limits"
          title={`Key budget — ${names[keyId] ?? keyId}`}
          models={models}
          load={() => orNull(client.keys.limits.get(keyId))}
          save={b => wrap(client.keys.limits.set(keyId, b))}
          remove={() => wrap(client.keys.limits.delete(keyId))}
          resetSpend={() => wrap(client.keys.limits.resetSpend(keyId))}
        />
      )}
    </div>
  );
}
