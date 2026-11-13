import { useEffect, useRef, useState } from 'react';
import { quietClient } from '../sdk-client';
import { checkKey, checkWithServer, createCheckLedger, type CheckKind, type Verdict } from './server-check';

/** How long a body must stay unchanged before it is checked, as Compare's cost preview always waited. */
const DEBOUNCE_MS = 400;

/** One body the page would send: Single mode's one, or one per Compare model (`lane`). */
export interface CheckItem {
  lane: string;
  kind: CheckKind;
  body: object;
}

/** What the page shows for one lane's current body. */
export interface LaneCheck {
  /** The server's reasons it would refuse the body, in its words. */
  problems: string[];
  /** The quote for the body, when the server gave one. */
  costUsd?: number;
  /** No verdict on this body yet: it blocks nothing meanwhile, as the submit validates anyway. */
  checking: boolean;
}

/**
 * Pre-submit checks from the server (E11b; costs in `server-check.ts`): each
 * body goes to its family's quote route once it has stayed unchanged for
 * 400 ms; a call superseded by a newer body in its lane is aborted, and each
 * verdict is kept under its body's key, so the same body is not asked twice
 * and a late answer for an old body never shows for the current one. Empty
 * `items` stop checking (no model, a run in flight, segmentation): calls in
 * flight are aborted.
 */
export function useServerChecks(items: CheckItem[]): Record<string, LaneCheck> {
  const keyed = items.map(item => ({ ...item, key: checkKey(item.kind, JSON.stringify(item.body)) }));
  const [verdicts, setVerdicts] = useState<ReadonlyMap<string, Verdict>>(() => new Map());
  const [ledger] = useState(() => createCheckLedger((_key, _verdict, all) => setVerdicts(new Map(all))));

  // The bodies of this render, for the debounced call to send.
  const latest = useRef(keyed);
  useEffect(() => { latest.current = keyed; });

  const asked = keyed.map(item => `${item.lane}=${item.key}`).join('|');
  useEffect(() => {
    if (latest.current.length === 0) {
      ledger.stop();
      return;
    }
    const timer = setTimeout(() => {
      for (const item of latest.current) {
        if (ledger.knows(item.key)) continue;
        void ledger.check(item.lane, item.key, signal => checkWithServer(item.kind, item.body, quietClient, signal));
      }
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [asked, ledger]);

  // Leaving the page stops whatever is still being asked.
  useEffect(() => () => ledger.stop(), [ledger]);

  const out: Record<string, LaneCheck> = {};
  for (const item of keyed) {
    const v = verdicts.get(item.key);
    out[item.lane] = { problems: v?.problems ?? [], costUsd: v?.costUsd, checking: v === undefined };
  }
  return out;
}
