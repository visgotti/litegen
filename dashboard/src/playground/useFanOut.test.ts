import { describe, expect, it, vi } from 'vitest';

// useFanOut pulls in the shared SDK client, whose module body reads
// localStorage; these tests cover the pure run-ownership helper, so the real
// client is never needed (and would throw under the node test environment).
vi.mock('../sdk-client', () => ({ client: {} }));

const { createRunEpoch, MODEL3D_POLL_TIMEOUT_MS } = await import('./useFanOut');

describe('createRunEpoch', () => {
  it('hands out a live controller for the first run', () => {
    const epoch = createRunEpoch();
    const ctrl = epoch.begin();
    expect(ctrl.signal.aborted).toBe(false);
    expect(epoch.owns(ctrl)).toBe(true);
  });

  it('aborts the run it supersedes', () => {
    const epoch = createRunEpoch();
    const first = epoch.begin();
    const second = epoch.begin();
    expect(first.signal.aborted).toBe(true);
    expect(second.signal.aborted).toBe(false);
  });

  it('disowns a superseded run so its late results cannot patch the tiles', () => {
    const epoch = createRunEpoch();
    const first = epoch.begin();
    const second = epoch.begin();
    expect(epoch.owns(first)).toBe(false);
    expect(epoch.owns(second)).toBe(true);
  });

  it('keeps ownership through a cancel, so the cancelled run still settles the UI', () => {
    const epoch = createRunEpoch();
    const ctrl = epoch.begin();
    epoch.abort();
    expect(ctrl.signal.aborted).toBe(true);
    expect(epoch.owns(ctrl)).toBe(true);
  });

  it('tolerates a cancel before any run has started', () => {
    const epoch = createRunEpoch();
    expect(() => epoch.abort()).not.toThrow();
  });

  it('never reports a foreign controller as the current run', () => {
    const epoch = createRunEpoch();
    epoch.begin();
    expect(epoch.owns(new AbortController())).toBe(false);
  });
});

describe('MODEL3D_POLL_TIMEOUT_MS', () => {
  // The SDK default is 5 minutes, which a queued vendor job outlives — the
  // Playground must pass its own budget rather than inherit that one.
  it('is longer than the SDK default of five minutes', () => {
    expect(MODEL3D_POLL_TIMEOUT_MS).toBeGreaterThan(5 * 60_000);
  });

  // The backend reaps a stuck generation at MAX_ACTIVE_AGE_SECS (2 hours), so
  // polling beyond that would wait on a row the server has already failed.
  it('stops well before the backend reaps the generation at two hours', () => {
    expect(MODEL3D_POLL_TIMEOUT_MS).toBeLessThan(2 * 60 * 60_000);
  });
});
