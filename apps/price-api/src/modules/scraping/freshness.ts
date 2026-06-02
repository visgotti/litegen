import { Freshness } from '../../common/enums';

/**
 * Pure freshness-transition logic, isolated from the database so it can be
 * exhaustively unit-tested. Encodes the rule chosen in the design: a failed
 * refresh never destroys the served price — it flips the component to `stale`,
 * and only after `staleAfterFailures` consecutive misses does it become `failed`.
 */

export interface FreshnessState {
  freshness: Freshness;
  consecutiveFailures: number;
}

/** State after a successful refresh: fresh, failure counter reset. */
export function onSuccess(): FreshnessState {
  return { freshness: Freshness.FRESH, consecutiveFailures: 0 };
}

/** State after a failed refresh, given the current state and the threshold. */
export function onFailure(current: FreshnessState, staleAfterFailures: number): FreshnessState {
  const consecutiveFailures = current.consecutiveFailures + 1;
  const freshness =
    consecutiveFailures >= staleAfterFailures ? Freshness.FAILED : Freshness.STALE;
  return { freshness, consecutiveFailures };
}
