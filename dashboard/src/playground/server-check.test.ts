import { describe, expect, it, vi } from 'vitest';
import { checkWithServer, createCheckLedger, verdictOf, type Quoter, type Verdict } from './server-check';

// The Playground's pre-submit checks ask the server (E11b): the exact body
// Generate would send goes to the family's quote route, which runs the same
// validation as the generation and calls no vendor. Only a refusal of the
// request itself is a verdict; anything else blocks nothing, and the submit
// validates anyway.

/** An SDK error as `LiteGenAPIError` carries it (fields are duck-typed). */
const apiError = (status: number, type: string, code: string, message: string) =>
  Object.assign(new Error(message), { status, type, code });

/** A quoter whose every route answers with `answer` (a quote, or a thrown error). */
function quoter(answer: { total_cost_usd?: number } | Error): Quoter & { calls: string[] } {
  const calls: string[] = [];
  const route = (name: string) => ({
    estimateCost: vi.fn(async () => {
      calls.push(name);
      if (answer instanceof Error) throw answer;
      return answer;
    }),
  });
  return { images: route('images'), videos: route('videos'), models3d: route('models3d'), calls };
}

describe('checkWithServer', () => {
  it("a validation refusal is the problem, in the server's words", async () => {
    const q = quoter(apiError(400, 'validation_error', 'source_model_required',
      "'mock/rig-3d' is a rig operation and needs source_model: the mesh to work on"));
    expect(await checkWithServer('model3d', { model: 'mock/rig-3d' }, q))
      .toEqual({ problems: ["'mock/rig-3d' is a rig operation and needs source_model: the mesh to work on"] });
  });

  it('a model gone from the catalog, or retired, is a problem too', async () => {
    expect((await checkWithServer('image', {}, quoter(apiError(404, 'validation_error', 'model_not_found', "model 'x' not found")))).problems)
      .toEqual(["model 'x' not found"]);
    expect((await checkWithServer('image', {}, quoter(apiError(410, 'validation_error', 'model_retired', "model 'x' was retired")))).problems)
      .toEqual(["model 'x' was retired"]);
  });

  it('a model whose provider has no key is not held back: the run itself says so', async () => {
    const q = quoter(apiError(400, 'provider_not_configured', 'provider_not_configured', "Provider 'fal' is not configured"));
    expect(await checkWithServer('model3d', {}, q)).toEqual({ problems: [] });
  });

  it('a quote is no problem, and carries its price', async () => {
    expect(await checkWithServer('image', {}, quoter({ total_cost_usd: 0.04 }))).toEqual({ problems: [], costUsd: 0.04 });
  });

  it('gives no verdict when the server could not be asked or did not judge the request', async () => {
    for (const failure of [
      new TypeError('Failed to fetch'),
      apiError(429, 'rate_limit_error', 'rate_limited', 'slow down'),
      // The extractors answer `validation_error` while at capacity: load, not a verdict.
      apiError(503, 'validation_error', 'server_at_capacity', 'server at capacity, retry shortly'),
      apiError(413, 'validation_error', 'body_too_large', 'the request body is over 25 MiB'),
      apiError(401, 'authentication_error', 'not_authenticated', 'Not logged in'),
      apiError(403, 'forbidden_permission', 'forbidden_permission', "Permission 'generation:create' required"),
      apiError(502, 'hook_error', 'hook_refused', 'the pre_call hook refused'),
    ]) {
      expect(await checkWithServer('image', {}, quoter(failure)), String(failure)).toEqual({ problems: [] });
    }
  });

  it("asks the family's own quote route", async () => {
    for (const [kind, route] of [['image', 'images'], ['video', 'videos'], ['model3d', 'models3d']] as const) {
      const q = quoter({ total_cost_usd: 1 });
      await checkWithServer(kind, { model: 'm' }, q);
      expect(q.calls).toEqual([route]);
    }
  });
});

describe('verdictOf', () => {
  it('reads only a refusal of the request: a 400 validation_error, a 404 or a 410', () => {
    expect(verdictOf(apiError(400, 'validation_error', 'prompt_required', 'prompt is required'))).toEqual({ problems: ['prompt is required'] });
    expect(verdictOf(apiError(400, 'invalid_request_error', 'x', 'odd'))).toEqual({ problems: [] });
    expect(verdictOf('not an error')).toEqual({ problems: [] });
  });
});

/** A call the test answers when it chooses, ignoring the abort as a fetch that already landed would. */
function deferred() {
  let answer!: (v: Verdict) => void;
  const promise = new Promise<Verdict>(resolve => { answer = resolve; });
  return { promise, answer };
}

describe('the keyed verdict ledger', () => {
  const refused: Verdict = { problems: ['prompt is required'] };
  const fine: Verdict = { problems: [], costUsd: 0.04 };

  it('keeps each answer under the body it was asked about: a late answer for an old body is never the current verdict', async () => {
    const shown: Array<[string, Verdict]> = [];
    const ledger = createCheckLedger((key, v) => shown.push([key, v]));
    const a = deferred();
    const askA = ledger.check('single', 'A', () => a.promise);
    const askB = ledger.check('single', 'B', async () => fine);   // supersedes A
    await askB;
    a.answer(refused);                                           // A lands late
    await askA;
    expect(ledger.verdict('B')).toEqual(fine);
    expect(ledger.verdict('A'), 'the superseded call records nothing').toBeUndefined();
    expect(shown).toEqual([['B', fine]]);
  });

  it('gives no verdict for an aborted call, and asks again next time', async () => {
    const ledger = createCheckLedger(() => {});
    const a = deferred();
    let seen: AbortSignal | undefined;
    const asked = ledger.check('single', 'A', signal => { seen = signal; return a.promise; });
    ledger.stop();
    expect(seen?.aborted).toBe(true);
    a.answer(refused);
    await asked;
    expect(ledger.knows('A')).toBe(false);
    const again = vi.fn(async () => fine);
    await ledger.check('single', 'A', again);
    expect(again).toHaveBeenCalledTimes(1);
    expect(ledger.verdict('A')).toEqual(fine);
  });

  it('reuses the verdict of a body already checked, and asks once for a body in flight', async () => {
    const ledger = createCheckLedger(() => {});
    const ask = vi.fn(async () => refused);
    await Promise.all([ledger.check('single', 'A', ask), ledger.check('single', 'A', ask)]);
    await ledger.check('single', 'A', ask);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(ledger.verdict('A')).toEqual(refused);
  });

  it('checks each Compare model in its own lane: one model does not cancel another', async () => {
    const ledger = createCheckLedger(() => {});
    const a = deferred();
    let seenA: AbortSignal | undefined;
    const askA = ledger.check('model-a', 'A', signal => { seenA = signal; return a.promise; });
    await ledger.check('model-b', 'B', async () => fine);
    expect(seenA?.aborted).toBe(false);
    a.answer(refused);
    await askA;
    expect(ledger.verdict('A')).toEqual(refused);
    expect(ledger.verdict('B')).toEqual(fine);
  });

  it('remembers a bounded number of bodies, dropping the oldest', async () => {
    const ledger = createCheckLedger(() => {}, 2);
    for (const key of ['A', 'B', 'C']) await ledger.check('single', key, async () => fine);
    expect(ledger.knows('A')).toBe(false);
    expect(ledger.knows('B') && ledger.knows('C')).toBe(true);
  });
});
