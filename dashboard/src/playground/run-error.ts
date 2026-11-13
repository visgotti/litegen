/**
 * What a failed run says, and how long the server asks the page to wait
 * before trying again: a 503 `circuit_open` (every deployment of a model
 * route is cooling down; no vendor was called) or another 503 or 429 that
 * sends `Retry-After`. Pure: fields are duck-typed, as `LiteGenAPIError`
 * carries them (`retryAfterMs` is its reading of `Retry-After`).
 */
export function retryWaitMs(err: unknown): number | undefined {
  const e = (typeof err === 'object' && err !== null ? err : {}) as { status?: unknown; retryAfterMs?: unknown };
  if (e.status !== 503 && e.status !== 429) return undefined;
  return typeof e.retryAfterMs === 'number' && e.retryAfterMs > 0 ? e.retryAfterMs : undefined;
}

/** The error a run shows: the server's message, and when to try again when it says. */
export function runErrorText(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err);
  const wait = retryWaitMs(err);
  return wait === undefined ? message : `${message} Try again in ${Math.ceil(wait / 1000)} s.`;
}
