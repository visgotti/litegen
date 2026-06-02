import { useEffect, useRef } from 'react';

export function useAutoRefresh(callback: () => void, intervalMs: number, enabled: boolean) {
  const savedCallback = useRef(callback);
  useEffect(() => { savedCallback.current = callback; }, [callback]);
  useEffect(() => {
    if (!enabled) return;
    const id = window.setInterval(() => savedCallback.current(), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs, enabled]);
}
