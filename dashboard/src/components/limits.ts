// Pure helpers behind the Budgets, End users and Spend pages: turning a
// limits form into a `LimitsInput`, describing a budget's state, and shaping
// spend rows for the bar table. Kept free of React so they unit-test in node.
import type { EntityLimits, LimitsInput, SpendReportRow } from '@litegen/sdk';

/** Budget windows offered in the editor, in LiteLLM's duration format. */
export const DURATION_OPTIONS: { value: string; label: string }[] = [
  { value: '', label: 'Lifetime (never resets)' },
  { value: '1h', label: 'Hourly' },
  { value: '1d', label: 'Daily' },
  { value: '1w', label: 'Weekly' },
  { value: '30d', label: 'Every 30 days' },
  { value: '1mo', label: 'Monthly' },
];

/** The editor's raw field values: everything is a string until submitted. */
export interface LimitsForm {
  maxBudget: string;
  softBudget: string;
  duration: string;
  rpm: string;
  maxParallel: string;
  /** Empty = every model allowed. */
  allowedModels: string[];
}

export const EMPTY_FORM: LimitsForm = {
  maxBudget: '',
  softBudget: '',
  duration: '',
  rpm: '',
  maxParallel: '',
  allowedModels: [],
};

export function formFromLimits(l: EntityLimits | null | undefined): LimitsForm {
  if (!l) return { ...EMPTY_FORM };
  const num = (v: number | null | undefined) => (v === null || v === undefined ? '' : String(v));
  return {
    maxBudget: num(l.max_budget_usd),
    softBudget: num(l.soft_budget_usd),
    duration: l.budget_duration ?? '',
    rpm: num(l.rpm_limit),
    maxParallel: num(l.max_parallel_requests),
    allowedModels: l.allowed_models ?? [],
  };
}

/**
 * Validate the form and build the request body. Returns an error message
 * instead when a field is invalid, mirroring the server's own checks so the
 * user sees the problem before the round trip.
 */
export function limitsFromForm(f: LimitsForm, metadata?: unknown): LimitsInput | string {
  const money = (raw: string, name: string): number | null | string => {
    if (raw.trim() === '') return null;
    const n = Number(raw);
    if (!Number.isFinite(n) || n < 0) return `${name} must be a non-negative number`;
    return n;
  };
  const count = (raw: string, name: string): number | null | string => {
    if (raw.trim() === '') return null;
    const n = Number(raw);
    if (!Number.isInteger(n) || n < 1) return `${name} must be a whole number of at least 1`;
    return n;
  };
  const max = money(f.maxBudget, 'Max budget');
  if (typeof max === 'string') return max;
  const soft = money(f.softBudget, 'Soft budget');
  if (typeof soft === 'string') return soft;
  if (max !== null && soft !== null && soft > max) return 'Soft budget must not exceed the max budget';
  const rpm = count(f.rpm, 'RPM limit');
  if (typeof rpm === 'string') return rpm;
  const parallel = count(f.maxParallel, 'Max parallel requests');
  if (typeof parallel === 'string') return parallel;
  return {
    max_budget_usd: max,
    soft_budget_usd: soft,
    budget_duration: f.duration || null,
    rpm_limit: rpm,
    max_parallel_requests: parallel,
    allowed_models: f.allowedModels.length > 0 ? [...f.allowedModels] : null,
    metadata: (metadata ?? null) as LimitsInput['metadata'],
  };
}

export function formatUsd(v: number | null | undefined): string {
  if (v === null || v === undefined) return '—';
  if (v !== 0 && Math.abs(v) < 0.01) return `$${v.toFixed(4)}`;
  return `$${v.toFixed(2)}`;
}

/** Share of the hard budget used, 0–100 (null for an uncapped budget). */
export function budgetUsedPercent(l: Pick<EntityLimits, 'spend_usd' | 'max_budget_usd'>): number | null {
  if (l.max_budget_usd === null || l.max_budget_usd === undefined) return null;
  if (l.max_budget_usd <= 0) return 100;
  return Math.min(100, Math.max(0, (l.spend_usd / l.max_budget_usd) * 100));
}

export type BudgetState = 'ok' | 'warning' | 'exhausted' | 'uncapped';

/** Traffic-light state: warning once the soft budget is crossed or 80% is used. */
export function budgetState(l: Pick<EntityLimits, 'spend_usd' | 'max_budget_usd' | 'soft_budget_usd'>): BudgetState {
  const pct = budgetUsedPercent(l);
  if (pct === null) {
    if (l.soft_budget_usd !== null && l.soft_budget_usd !== undefined && l.spend_usd >= l.soft_budget_usd) return 'warning';
    return 'uncapped';
  }
  if (pct >= 100) return 'exhausted';
  if (pct >= 80 || (l.soft_budget_usd != null && l.spend_usd >= l.soft_budget_usd)) return 'warning';
  return 'ok';
}

export function durationLabel(d: string | null | undefined): string {
  if (!d) return 'Lifetime';
  return DURATION_OPTIONS.find(o => o.value === d)?.label ?? d;
}

/** "resets in 3d 4h" style countdown; '' for a lifetime budget. */
export function resetsIn(resetAt: string | null | undefined, now: Date = new Date()): string {
  if (!resetAt) return '';
  const ms = new Date(resetAt).getTime() - now.getTime();
  if (!Number.isFinite(ms)) return '';
  if (ms <= 0) return 'resets now';
  const mins = Math.floor(ms / 60_000);
  const d = Math.floor(mins / 1440);
  const h = Math.floor((mins % 1440) / 60);
  const m = mins % 60;
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${Math.max(m, 1)}m`;
}

/** Human label for an entity row in the all-limits table. */
export function entityLabel(l: Pick<EntityLimits, 'entity_type' | 'entity_id'>, names: Record<string, string> = {}): string {
  const id = l.entity_type === 'member' || l.entity_type === 'end_user'
    ? l.entity_id.slice(l.entity_id.indexOf(':') + 1)
    : l.entity_id;
  return names[id] ?? id;
}

/** Treat a 404 as "no limits set"; rethrow anything else. */
export async function orNull<T>(p: Promise<T>): Promise<T | null> {
  try {
    return await p;
  } catch (e) {
    if ((e as { status?: number }).status === 404) return null;
    throw e;
  }
}

// ─── Spend ───────────────────────────────────────────────────────────────────

export const GROUP_BY_OPTIONS = [
  { value: 'day', label: 'Day' },
  { value: 'model', label: 'Model' },
  { value: 'provider', label: 'Provider' },
  { value: 'media_type', label: 'Media type' },
  { value: 'app', label: 'Application' },
  { value: 'key', label: 'API key' },
  { value: 'user', label: 'User' },
  { value: 'end_user', label: 'End user' },
  { value: 'tag', label: 'Tag' },
  { value: 'status', label: 'Status' },
] as const;

export interface SpendBar {
  label: string;
  requests: number;
  failed: number;
  cost: number;
  /** Width of the bar, 0–100, relative to the costliest row. */
  width: number;
}

export function spendBars(rows: SpendReportRow[], names: Record<string, string> = {}): SpendBar[] {
  const top = rows.reduce((m, r) => Math.max(m, r.cost_usd), 0);
  return rows.map(r => ({
    label: r.group === null || r.group === undefined ? '(none)' : names[r.group] ?? r.group,
    requests: r.requests,
    failed: r.failed_requests,
    cost: r.cost_usd,
    width: top > 0 ? (r.cost_usd / top) * 100 : 0,
  }));
}

/** `YYYY-MM-DD` for `daysAgo` days before `now` (UTC). */
export function isoDay(daysAgo = 0, now: Date = new Date()): string {
  const d = new Date(now.getTime() - daysAgo * 86_400_000);
  return d.toISOString().slice(0, 10);
}
