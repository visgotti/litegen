export interface Toast {
  id: number;
  message: string;
  level: 'info' | 'warning' | 'error';
  ttlMs: number;
}

let counter = 0;
let listeners: Array<(t: Toast) => void> = [];

export function subscribe(fn: (t: Toast) => void): () => void {
  listeners.push(fn);
  return () => {
    listeners = listeners.filter(l => l !== fn);
  };
}

export function showToast(message: string, level: 'info' | 'warning' | 'error' = 'info'): void {
  const toast: Toast = { id: counter++, message, level, ttlMs: 4000 };
  listeners.forEach(fn => fn(toast));
}
