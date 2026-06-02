import { Freshness } from '../../common/enums';
import { onFailure, onSuccess } from './freshness';

describe('freshness transitions', () => {
  it('success resets to fresh with zero failures', () => {
    expect(onSuccess()).toEqual({ freshness: Freshness.FRESH, consecutiveFailures: 0 });
  });

  it('first failure degrades fresh -> stale (keeps the price)', () => {
    const next = onFailure({ freshness: Freshness.FRESH, consecutiveFailures: 0 }, 3);
    expect(next).toEqual({ freshness: Freshness.STALE, consecutiveFailures: 1 });
  });

  it('stays stale until the threshold, then becomes failed', () => {
    let state = { freshness: Freshness.FRESH, consecutiveFailures: 0 };
    state = onFailure(state, 3); // 1 -> stale
    expect(state.freshness).toBe(Freshness.STALE);
    state = onFailure(state, 3); // 2 -> stale
    expect(state.freshness).toBe(Freshness.STALE);
    state = onFailure(state, 3); // 3 -> failed
    expect(state.freshness).toBe(Freshness.FAILED);
    expect(state.consecutiveFailures).toBe(3);
  });

  it('a success after failures recovers to fresh', () => {
    const failed = { freshness: Freshness.FAILED, consecutiveFailures: 5 };
    void failed;
    expect(onSuccess().freshness).toBe(Freshness.FRESH);
  });
});
