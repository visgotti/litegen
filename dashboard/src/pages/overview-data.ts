// The Overview page's chart data, kept free of React so it is unit-tested.
import type { RequestLog } from '@litegen/sdk';

/** The day's request costs summed per hour of the day (local time), every
 *  digit kept: the chart shows them with `formatCost`, which drops binary
 *  noise, never a real fraction of a cent. */
export function hourlyCost(logs: RequestLog[]): { hour: string; cost: number }[] {
  const buckets = Array.from({ length: 24 }, (_, i) => ({ hour: String(i).padStart(2, '0') + ':00', cost: 0 }));
  for (const log of logs) {
    const h = new Date(log.created_at).getHours();
    buckets[h].cost += log.cost_usd || 0;
  }
  return buckets;
}
