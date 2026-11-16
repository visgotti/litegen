// A key's quota on the Keys page, kept free of React so it is unit-tested.
import { formatCost } from '../components/model-detail';

/** What the key has spent of its quota, in dollars: `token_quota` is a USD
 *  budget cap and `tokens_used` the USD spent (the wire names predate that);
 *  a dash for a key with no cap. */
export function quotaText(key: { token_quota?: number | null; tokens_used?: number | null }): string {
  if (key.token_quota == null) return '—';
  return `${formatCost(key.tokens_used ?? 0)} / ${formatCost(key.token_quota)}`;
}
