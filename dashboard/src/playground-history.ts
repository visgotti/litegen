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
