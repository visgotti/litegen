import { describe, expect, it } from 'vitest';
import { retryWaitMs, runErrorText } from './run-error';

/** An SDK error as `LiteGenAPIError` carries it (fields duck-typed). */
const apiError = (status: number, code: string, message: string, retryAfterMs?: number, retryable?: boolean) =>
  Object.assign(new Error(message), { status, type: 'api_error', code, retryAfterMs, retryable });

describe('a run the server asks to retry later', () => {
  // A model route whose every deployment's breaker is open: no vendor was
  // called, and Retry-After says when the first one closes (late-a).
  const circuitOpen = apiError(503, 'circuit_open', "every deployment of 'mock/visual-image-gen' is cooling down", 2500, true);

  it('says when to try again, from Retry-After', () => {
    expect(retryWaitMs(circuitOpen)).toBe(2500);
    expect(runErrorText(circuitOpen)).toBe("every deployment of 'mock/visual-image-gen' is cooling down Try again in 3 s.");
  });

  it('waits out any 503 or 429 that names its wait, and nothing else', () => {
    expect(retryWaitMs(apiError(429, 'rate_limited', 'slow down', 1000))).toBe(1000);
    expect(retryWaitMs(apiError(503, 'server_at_capacity', 'busy', 1000))).toBe(1000);
    expect(retryWaitMs(apiError(503, 'circuit_open', 'cooling', undefined, true))).toBeUndefined();
    expect(retryWaitMs(apiError(400, 'prompt_required', 'prompt is required', 1000))).toBeUndefined();
    expect(retryWaitMs(new TypeError('Failed to fetch'))).toBeUndefined();
    expect(runErrorText(apiError(400, 'prompt_required', 'prompt is required'))).toBe('prompt is required');
  });
});
