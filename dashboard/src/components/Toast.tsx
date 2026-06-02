import { useEffect, useState } from 'react';
import { subscribe } from './toast-store';
import type { Toast } from './toast-store';

export default function ToastContainer() {
  const [toasts, setToasts] = useState<Toast[]>([]);

  useEffect(() => {
    const unsub = subscribe((t: Toast) => {
      setToasts(prev => [...prev, t]);
      window.setTimeout(() => {
        setToasts(prev => prev.filter(x => x.id !== t.id));
      }, t.ttlMs);
    });
    return unsub;
  }, []);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-container">
      {toasts.map(t => (
        <div
          key={t.id}
          className={`toast ${t.level}`}
          data-testid={`toast-${t.level}`}
        >
          {t.message}
        </div>
      ))}
    </div>
  );
}
