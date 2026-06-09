export interface PlaygroundHistoryEntry {
  id: string;
  model: string;
  prompt: string;
  timestamp: string;
  requestBody: Record<string, unknown>;
  responseBody: Record<string, unknown>;
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
  const capped = history.slice(-HISTORY_CAP);
  localStorage.setItem(HISTORY_KEY, JSON.stringify(capped));
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
  localStorage.setItem(MULTI_KEY, JSON.stringify(history.slice(-MULTI_CAP)));
}

export function removeMultiHistory(id: string): void {
  const history = getMultiHistory().filter(e => e.id !== id);
  localStorage.setItem(MULTI_KEY, JSON.stringify(history));
}
