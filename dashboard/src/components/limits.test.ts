import { describe, it, expect } from 'vitest';
import type { EntityLimits } from '@litegen/sdk';
import {
  EMPTY_FORM,
  budgetState,
  budgetUsedPercent,
  durationLabel,
  entityLabel,
  formFromLimits,
  formatUsd,
  isoDay,
  limitsFromForm,
  resetsIn,
  spendBars,
} from './limits';

const row = (over: Partial<EntityLimits> = {}): EntityLimits => ({
  entity_type: 'org',
  entity_id: 'o1',
  org_id: 'o1',
  max_budget_usd: 10,
  soft_budget_usd: 8,
  budget_duration: '30d',
  spend_usd: 2.5,
  budget_reset_at: '2026-11-01T00:00:00Z',
  soft_budget_alerted_at: null,
  allowed_models: ['mock/image-gen'],
  max_parallel_requests: 4,
  rpm_limit: 60,
  metadata: null,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
  ...over,
});

describe('limits form', () => {
  it('round-trips a stored row', () => {
    const f = formFromLimits(row());
    expect(f).toEqual({ maxBudget: '10', softBudget: '8', duration: '30d', rpm: '60', maxParallel: '4', allowedModels: ['mock/image-gen'] });
    expect(limitsFromForm(f)).toEqual({
      max_budget_usd: 10,
      soft_budget_usd: 8,
      budget_duration: '30d',
      rpm_limit: 60,
      max_parallel_requests: 4,
      allowed_models: ['mock/image-gen'],
      metadata: null,
    });
  });

  it('turns blanks into nulls', () => {
    expect(formFromLimits(null)).toEqual(EMPTY_FORM);
    expect(limitsFromForm(EMPTY_FORM)).toEqual({
      max_budget_usd: null,
      soft_budget_usd: null,
      budget_duration: null,
      rpm_limit: null,
      max_parallel_requests: null,
      allowed_models: null,
      metadata: null,
    });
  });

  it('keeps metadata it is given', () => {
    const out = limitsFromForm(EMPTY_FORM, { team: 'growth' });
    expect(typeof out === 'object' && out.metadata).toEqual({ team: 'growth' });
  });

  it('rejects what the server would reject', () => {
    expect(limitsFromForm({ ...EMPTY_FORM, maxBudget: '-1' })).toMatch(/Max budget/);
    expect(limitsFromForm({ ...EMPTY_FORM, maxBudget: 'abc' })).toMatch(/Max budget/);
    expect(limitsFromForm({ ...EMPTY_FORM, softBudget: '-2' })).toMatch(/Soft budget/);
    expect(limitsFromForm({ ...EMPTY_FORM, maxBudget: '5', softBudget: '6' })).toMatch(/must not exceed/);
    expect(limitsFromForm({ ...EMPTY_FORM, rpm: '0' })).toMatch(/RPM/);
    expect(limitsFromForm({ ...EMPTY_FORM, rpm: '1.5' })).toMatch(/RPM/);
    expect(limitsFromForm({ ...EMPTY_FORM, maxParallel: '0' })).toMatch(/parallel/);
  });
});

describe('budget state', () => {
  it('computes the used share', () => {
    expect(budgetUsedPercent(row())).toBe(25);
    expect(budgetUsedPercent(row({ spend_usd: 50 }))).toBe(100);
    expect(budgetUsedPercent(row({ max_budget_usd: null }))).toBeNull();
    expect(budgetUsedPercent(row({ max_budget_usd: 0, spend_usd: 0 }))).toBe(100);
  });

  it('maps to a traffic light', () => {
    expect(budgetState(row())).toBe('ok');
    expect(budgetState(row({ spend_usd: 8.5 }))).toBe('warning');
    expect(budgetState(row({ spend_usd: 8.1, soft_budget_usd: null }))).toBe('warning');
    expect(budgetState(row({ spend_usd: 10 }))).toBe('exhausted');
    expect(budgetState(row({ max_budget_usd: null, soft_budget_usd: null }))).toBe('uncapped');
    expect(budgetState(row({ max_budget_usd: null, soft_budget_usd: 1, spend_usd: 2 }))).toBe('warning');
  });
});

describe('formatting', () => {
  it('formats money', () => {
    expect(formatUsd(null)).toBe('—');
    expect(formatUsd(0)).toBe('$0.00');
    expect(formatUsd(5.5)).toBe('$5.50');
    expect(formatUsd(0.004)).toBe('$0.0040');
  });

  it('labels durations', () => {
    expect(durationLabel(null)).toBe('Lifetime');
    expect(durationLabel('1mo')).toBe('Monthly');
    expect(durationLabel('12h')).toBe('12h');
  });

  it('counts down to a reset', () => {
    const now = new Date('2026-10-01T00:00:00Z');
    expect(resetsIn(null, now)).toBe('');
    expect(resetsIn('garbage', now)).toBe('');
    expect(resetsIn('2026-09-30T00:00:00Z', now)).toBe('resets now');
    expect(resetsIn('2026-10-04T04:00:00Z', now)).toBe('resets in 3d 4h');
    expect(resetsIn('2026-10-01T02:30:00Z', now)).toBe('resets in 2h 30m');
    expect(resetsIn('2026-10-01T00:00:20Z', now)).toBe('resets in 1m');
  });

  it('labels entities without their org prefix', () => {
    expect(entityLabel({ entity_type: 'member', entity_id: 'o1:u1' }, { u1: 'jane@x.test' })).toBe('jane@x.test');
    expect(entityLabel({ entity_type: 'end_user', entity_id: 'o1:cust:42' })).toBe('cust:42');
    expect(entityLabel({ entity_type: 'app', entity_id: 'a1' })).toBe('a1');
  });
});

describe('spend bars', () => {
  it('scales widths to the costliest row and names groups', () => {
    const bars = spendBars(
      [
        { group: 'k1', requests: 4, failed_requests: 1, cost_usd: 10 },
        { group: null, requests: 1, failed_requests: 0, cost_usd: 2.5 },
      ],
      { k1: 'Prod key' },
    );
    expect(bars).toEqual([
      { label: 'Prod key', requests: 4, failed: 1, cost: 10, width: 100 },
      { label: '(none)', requests: 1, failed: 0, cost: 2.5, width: 25 },
    ]);
    expect(spendBars([{ group: 'x', requests: 1, failed_requests: 0, cost_usd: 0 }])[0]!.width).toBe(0);
  });

  it('formats days', () => {
    const now = new Date('2026-10-12T15:00:00Z');
    expect(isoDay(0, now)).toBe('2026-10-12');
    expect(isoDay(30, now)).toBe('2026-09-12');
  });
});
