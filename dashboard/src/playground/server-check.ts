/**
 * The Playground's pre-submit checks come from the server (E11b). The exact
 * body Generate would send goes to its family's quote route
 * (`POST /v1/{images,videos,models3d}/cost`), which runs the same validation
 * as the generation and calls no vendor. The server's refusal is the problem,
 * in its words; the page keeps no copy of its rules.
 *
 * What the checks cost: one quote per body the form settles on, 400 ms after
 * the last edit (`useServerCheck`). Each carries the whole body, uploaded
 * images inline included (up to the request limit), holds one of the server's
 * in-flight slots while it is read, and runs the operator's pre_call and
 * post_call hooks like any quote; Compare mode always did this for its cost
 * preview. A 402, 403 or 429 on a check raises no toast (`quietClient`).
 *
 * Pure, so its rules are unit-tested without a DOM.
 */

/** The families with a quote route. A segmentation has none: its form is judged by what is missing. */
export type CheckKind = 'image' | 'video' | 'model3d';

/** What the server said about one body. */
export interface Verdict {
  /** Why the server would refuse the body, in its words; empty when it would not, or did not say. */
  problems: string[];
  /** The quote, when the server gave one. */
  costUsd?: number;
}

type QuoteRoute = { estimateCost(body: never, signal?: AbortSignal): Promise<{ total_cost_usd?: number } | undefined> };

/** The quote routes, as the SDK client has them; a test passes a fake. */
export interface Quoter {
  images: QuoteRoute;
  videos: QuoteRoute;
  models3d: QuoteRoute;
}

const ROUTE: Record<CheckKind, keyof Quoter> = { image: 'images', video: 'videos', model3d: 'models3d' };

/** No verdict: the server was not asked, or said nothing about the request itself. */
const NO_VERDICT: Verdict = { problems: [] };

/**
 * What an error from a quote says about the request. Only a refusal of the
 * request itself is a verdict: a 400 `validation_error`, or a model that is
 * not in the catalog (404) or is retired (410). Anything else blocks nothing,
 * and the submit says why: a provider with no key (`provider_not_configured`,
 * so a "needs provider setup" model still runs and fails as before), the
 * server at capacity (a 503, though also typed `validation_error`), a body
 * too large, a 401, 403 or 429, a hook's refusal, a network error, an abort.
 * Fields are duck-typed, as `LiteGenAPIError` carries them.
 */
export function verdictOf(err: unknown): Verdict {
  const e = (typeof err === 'object' && err !== null ? err : {}) as { status?: unknown; type?: unknown; message?: unknown };
  const refusal = e.type === 'validation_error' && (e.status === 400 || e.status === 404 || e.status === 410);
  return refusal && typeof e.message === 'string' ? { problems: [e.message] } : NO_VERDICT;
}

/** Ask the family's quote route about `body`: the verdict, with the price when the server quoted one. */
export async function checkWithServer(kind: CheckKind, body: object, quoter: Quoter, signal?: AbortSignal): Promise<Verdict> {
  try {
    const quote = await quoter[ROUTE[kind]].estimateCost(body as never, signal);
    const cost = quote?.total_cost_usd;
    return typeof cost === 'number' ? { problems: [], costUsd: cost } : { problems: [] };
  } catch (err) {
    return verdictOf(err);
  }
}

/**
 * A short key for a body: the same body, the same key. A hash of its JSON
 * (cyrb53, both halves), since the JSON can carry megabytes of inline images.
 */
export function checkKey(kind: CheckKind, json: string): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < json.length; i++) {
    const ch = json.charCodeAt(i);
    h1 = Math.imul(h1 ^ ch, 2654435761);
    h2 = Math.imul(h2 ^ ch, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return `${kind}:${json.length}:${(h2 >>> 0).toString(36)}.${(h1 >>> 0).toString(36)}`;
}

/** The verdicts the page holds, by body key, and the call in flight per lane. */
export interface CheckLedger {
  /** The verdict the server gave the body `key`, once it has answered. */
  verdict(key: string): Verdict | undefined;
  /** Whether the body `key` has a verdict. */
  knows(key: string): boolean;
  /**
   * Ask about the body `key` in `lane` (one model), unless its verdict is
   * known or it is being asked now. A call in flight in the same lane for
   * another body is aborted first: superseded. An aborted call records
   * nothing, so its body is asked again when it comes back.
   */
  check(lane: string, key: string, ask: (signal: AbortSignal) => Promise<Verdict>): Promise<void>;
  /** Abort the calls in flight, in one lane or all of them (the page left, or a run started). */
  stop(lane?: string): void;
}

/**
 * The keyed verdict ledger behind `useServerCheck`: each body's verdict is
 * kept under its own key, so a late answer for an old body is never the
 * verdict on the current one; it keeps the last `limit` bodies.
 * `onVerdict` hears each verdict recorded, with every verdict held.
 */
export function createCheckLedger(
  onVerdict: (key: string, verdict: Verdict, verdicts: ReadonlyMap<string, Verdict>) => void,
  limit = 32,
): CheckLedger {
  const verdicts = new Map<string, Verdict>();
  const inFlight = new Map<string, { key: string; ctrl: AbortController }>();
  return {
    verdict: key => verdicts.get(key),
    knows: key => verdicts.has(key),
    async check(lane, key, ask) {
      if (verdicts.has(key)) return;
      const current = inFlight.get(lane);
      if (current?.key === key) return;
      current?.ctrl.abort();
      const ctrl = new AbortController();
      inFlight.set(lane, { key, ctrl });
      let verdict: Verdict;
      try {
        verdict = await ask(ctrl.signal);
      } catch {
        verdict = NO_VERDICT;
      }
      if (inFlight.get(lane)?.ctrl === ctrl) inFlight.delete(lane);
      if (ctrl.signal.aborted) return;
      verdicts.delete(key);
      verdicts.set(key, verdict);
      while (verdicts.size > limit) verdicts.delete(verdicts.keys().next().value as string);
      onVerdict(key, verdict, verdicts);
    },
    stop(lane) {
      for (const [l, call] of [...inFlight]) {
        if (lane !== undefined && l !== lane) continue;
        call.ctrl.abort();
        inFlight.delete(l);
      }
    },
  };
}
