import { useEffect, useState } from 'react';
import type { EntityLimits, LimitsInput, ModelInfo } from '@litegen/sdk';
import { showToast } from './toast-store';
import {
  DURATION_OPTIONS,
  budgetState,
  budgetUsedPercent,
  durationLabel,
  formFromLimits,
  formatUsd,
  limitsFromForm,
  resetsIn,
  type LimitsForm,
} from './limits';
import '../management.css';

interface Props {
  /** Prefix for data-testids, e.g. `org-limits`. */
  testid: string;
  title: string;
  /** Loads the current limits; resolves null when none are set (404). */
  load: () => Promise<EntityLimits | null>;
  save: (input: LimitsInput) => Promise<EntityLimits>;
  remove: () => Promise<unknown>;
  resetSpend: () => Promise<EntityLimits>;
  /** Models offered for the allowlist. */
  models?: ModelInfo[];
  readOnly?: boolean;
}

/** Spend bar for one limits row. */
export function BudgetMeter({ limits, testid }: { limits: EntityLimits; testid?: string }) {
  const pct = budgetUsedPercent(limits);
  const state = budgetState(limits);
  return (
    <div className="mg-meter" data-testid={testid}>
      <div className="mg-meter-text">
        <span data-testid={testid ? `${testid}-spend` : undefined}>{formatUsd(limits.spend_usd)}</span>
        <span className="mg-muted"> of {limits.max_budget_usd == null ? 'no cap' : formatUsd(limits.max_budget_usd)}</span>
        <span className="mg-muted"> · {durationLabel(limits.budget_duration)}</span>
        {limits.budget_reset_at && <span className="mg-muted"> · {resetsIn(limits.budget_reset_at)}</span>}
      </div>
      {pct !== null && (
        <div className="mg-meter-track">
          <div className={`mg-meter-fill mg-${state}`} style={{ width: `${pct}%` }} />
        </div>
      )}
    </div>
  );
}

export default function LimitsEditor({ testid, title, load, save, remove, resetSpend, models = [], readOnly }: Props) {
  const [current, setCurrent] = useState<EntityLimits | null>(null);
  const [form, setForm] = useState<LimitsForm>(formFromLimits(null));
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError('');
    load()
      .then(l => {
        if (cancelled) return;
        setCurrent(l);
        setForm(formFromLimits(l));
      })
      .catch(e => !cancelled && setError(e instanceof Error ? e.message : String(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
    // `load` is recreated by parents on every render. Parents remount the
    // editor (React `key`) when its target changes, so load once per mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [testid]);

  const set = (patch: Partial<LimitsForm>) => setForm(f => ({ ...f, ...patch }));

  const run = async (action: () => Promise<EntityLimits | null>, done: string) => {
    setBusy(true);
    setError('');
    try {
      const l = await action();
      setCurrent(l);
      setForm(formFromLimits(l));
      showToast(done, 'info');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onSave = () => {
    const body = limitsFromForm(form, current?.metadata);
    if (typeof body === 'string') {
      setError(body);
      return;
    }
    void run(() => save(body), `${title} saved`);
  };

  const toggleModel = (id: string) =>
    set({ allowedModels: form.allowedModels.includes(id) ? form.allowedModels.filter(m => m !== id) : [...form.allowedModels, id] });

  if (loading) return <div className="mg-card loading">Loading {title.toLowerCase()}…</div>;

  return (
    <div className="mg-card" data-testid={testid}>
      <div className="mg-card-head">
        <h3>{title}</h3>
        {current ? <span className="badge healthy" data-testid={`${testid}-state`}>limits set</span> : <span className="badge" data-testid={`${testid}-state`}>no limits</span>}
      </div>
      {error && <div className="alert alert-error" data-testid={`${testid}-error`}>{error}</div>}
      {current && <BudgetMeter limits={current} testid={`${testid}-meter`} />}
      {current?.soft_budget_alerted_at && (
        <div className="mg-muted mg-small" data-testid={`${testid}-alerted`}>Soft budget crossed {new Date(current.soft_budget_alerted_at).toLocaleString()}</div>
      )}
      <div className="mg-form-grid">
        <label>
          <span>Max budget (USD)</span>
          <input className="input" inputMode="decimal" placeholder="unlimited" value={form.maxBudget} disabled={readOnly}
            data-testid={`${testid}-max`} onChange={e => set({ maxBudget: e.target.value })} />
        </label>
        <label>
          <span>Soft budget alert (USD)</span>
          <input className="input" inputMode="decimal" placeholder="none" value={form.softBudget} disabled={readOnly}
            data-testid={`${testid}-soft`} onChange={e => set({ softBudget: e.target.value })} />
        </label>
        <label>
          <span>Budget window</span>
          <select className="input" value={form.duration} disabled={readOnly} data-testid={`${testid}-duration`}
            onChange={e => set({ duration: e.target.value })}>
            {DURATION_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
            {form.duration && !DURATION_OPTIONS.some(o => o.value === form.duration) && <option value={form.duration}>{form.duration}</option>}
          </select>
        </label>
        <label>
          <span>Requests / minute</span>
          <input className="input" inputMode="numeric" placeholder="unlimited" value={form.rpm} disabled={readOnly}
            data-testid={`${testid}-rpm`} onChange={e => set({ rpm: e.target.value })} />
        </label>
        <label>
          <span>Max parallel requests</span>
          <input className="input" inputMode="numeric" placeholder="unlimited" value={form.maxParallel} disabled={readOnly}
            data-testid={`${testid}-parallel`} onChange={e => set({ maxParallel: e.target.value })} />
        </label>
      </div>
      {models.length > 0 && (
        <details className="mg-models">
          <summary>
            Model allowlist: {form.allowedModels.length === 0 ? 'all models' : `${form.allowedModels.length} selected`}
          </summary>
          <div className="mg-model-list">
            {models.map(m => (
              <label key={m.id}>
                <input type="checkbox" checked={form.allowedModels.includes(m.id)} disabled={readOnly}
                  data-testid={`${testid}-model-${m.id}`} onChange={() => toggleModel(m.id)} />
                <span>{m.id}</span>
              </label>
            ))}
          </div>
        </details>
      )}
      {!readOnly && (
        <div className="mg-actions">
          <button className="btn btn-primary" disabled={busy} data-testid={`${testid}-save`} onClick={onSave}>Save limits</button>
          {current && (
            <>
              <button className="btn btn-secondary" disabled={busy} data-testid={`${testid}-reset`}
                onClick={() => void run(resetSpend, 'Spend reset')}>Reset spend</button>
              <button className="btn btn-danger" disabled={busy} data-testid={`${testid}-remove`}
                onClick={() => void run(async () => { await remove(); return null; }, `${title} removed`)}>Remove limits</button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
