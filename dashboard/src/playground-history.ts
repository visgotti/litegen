export interface PlaygroundHistoryEntry {
  id: string;
  model: string;
  prompt: string;
  timestamp: string;
  requestBody: Record<string, unknown>;
  responseBody: Record<string, unknown>;
  /** How many uploaded reference images `requestBody` lost to
   *  `withoutInlineRefs`; a Re-submit must ask for them, not run without. */
  strippedRefs?: number;
}

const HISTORY_KEY = 'litegen_playground_history';
const HISTORY_CAP = 10;

export function getPlaygroundHistory(): PlaygroundHistoryEntry[] {
  try {
    return JSON.parse(localStorage.getItem(HISTORY_KEY) ?? '[]');
  } catch {
    return [];
  }
}

export function pushPlaygroundHistory(entry: PlaygroundHistoryEntry): void {
  const history = getPlaygroundHistory();
  history.push(entry);
  // FIFO cap at 10
  store(HISTORY_KEY, history.slice(-HISTORY_CAP));
}

/** Write `entries`, dropping the oldest until they fit. History is a
 *  convenience: a full or unavailable store never fails the run being
 *  recorded, which has already been made (and billed). */
function store(key: string, entries: unknown[]): void {
  for (let from = 0; from < entries.length; from++) {
    try {
      localStorage.setItem(key, JSON.stringify(entries.slice(from)));
      return;
    } catch {
      // Too large for what is left: try without the oldest entry.
    }
  }
}

export function removePlaygroundHistory(id: string): void {
  const history = getPlaygroundHistory().filter(e => e.id !== id);
  localStorage.setItem(HISTORY_KEY, JSON.stringify(history));
}

// ─── Multi-model (Compare mode) history ──────────────────────────────────────

export interface MultiRunResult {
  model: string;
  request: Record<string, unknown>;
  response?: Record<string, unknown>;
  error?: string;
}

export interface MultiRunHistoryEntry {
  id: string;
  kind: 'multi';
  prompt: string;
  timestamp: string;
  models: string[];
  results: MultiRunResult[];
}

const MULTI_KEY = 'litegen_playground_multi_history';
const MULTI_CAP = 10;

export function getMultiHistory(): MultiRunHistoryEntry[] {
  try {
    return JSON.parse(localStorage.getItem(MULTI_KEY) ?? '[]');
  } catch {
    return [];
  }
}

export function pushMultiHistory(entry: MultiRunHistoryEntry): void {
  const history = getMultiHistory();
  history.push(entry);
  store(MULTI_KEY, history.slice(-MULTI_CAP));
}

export function removeMultiHistory(id: string): void {
  const history = getMultiHistory().filter(e => e.id !== id);
  localStorage.setItem(MULTI_KEY, JSON.stringify(history));
}
