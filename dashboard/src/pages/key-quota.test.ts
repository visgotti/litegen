import { describe, expect, it } from 'vitest';
import { quotaText } from './key-quota';

// A key's quota is a USD budget cap (`token_quota`, "USD budget cap") and
// what it has spent is USD too (`tokens_used`, "Running USD spent"): the Keys
// page shows both as money, every digit kept. The wire names stay.

describe('quotaText', () => {
  it('shows what the key spent of its cap, in dollars', () => {
    expect(quotaText({ token_quota: 100, tokens_used: 0.00033 })).toBe('$0.00033 / $100.00');
    expect(quotaText({ token_quota: 50, tokens_used: null })).toBe('$0.00 / $50.00');
  });

  it('shows a dash for a key with no cap', () => {
    expect(quotaText({ token_quota: null, tokens_used: 12 })).toBe('—');
    expect(quotaText({})).toBe('—');
  });
});
