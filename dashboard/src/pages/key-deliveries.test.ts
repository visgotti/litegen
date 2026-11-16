import { describe, expect, it } from 'vitest';
import { redeliverRowIds } from './key-deliveries';

// The deliveries panel lists attempts, newest first (GET /v1/keys/{id}/webhook-deliveries).
// A redeliver acts on a delivery (`delivery_id`), so its button sits once per
// delivery, on its newest attempt; an attempt recorded before migration 029
// has no delivery id, and nothing to redeliver by.

describe('redeliverRowIds', () => {
  it('puts one Redeliver on the newest attempt of each delivery', () => {
    const rows = [
      { id: 'a4', delivery_id: 'whd-1' },
      { id: 'a3', delivery_id: 'whd-2' },
      { id: 'a2', delivery_id: 'whd-1' },
      { id: 'a1', delivery_id: null },
    ];
    expect([...redeliverRowIds(rows)]).toEqual(['a4', 'a3']);
  });

  it('puts none on attempts with no delivery id', () => {
    expect(redeliverRowIds([{ id: 'a1', delivery_id: null }]).size).toBe(0);
  });
});
