import { describe, expect, it } from 'vitest';
import type { RequestLog } from '@litegen/sdk';
import { hourlyCost } from './overview-data';

// The Overview's hourly cost chart sums the day's request costs per hour. The
// sum keeps every digit (the chart's labels show it with formatCost): six
// fixed decimals rounded a sub-cent day of small charges away.

const at = (hour: number, cost: number) => ({ created_at: new Date(2026, 10, 15, hour, 5).toISOString(), cost_usd: cost }) as RequestLog;

describe('hourlyCost', () => {
  it('sums each hour, keeping every digit of small charges', () => {
    const buckets = hourlyCost([at(9, 0.0000004), at(9, 0.0000004), at(9, 0.0000004), at(10, 0.00033)]);
    expect(buckets).toHaveLength(24);
    expect(buckets[9].hour).toBe('09:00');
    expect(buckets[9].cost).toBeCloseTo(0.0000012, 12);
    expect(buckets[10].cost).toBe(0.00033);
    expect(buckets[0].cost).toBe(0);
  });
});
