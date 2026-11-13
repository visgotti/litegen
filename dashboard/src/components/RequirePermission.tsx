import { type ReactNode, useEffect, useState } from 'react';
import { Navigate } from 'react-router-dom';
import { client, getApiKey } from '../sdk-client';
import { permissionsFor } from './permissions';

interface Props {
  perm: string;
  children: ReactNode;
}

export default function RequirePermission({ perm, children }: Props) {
  // The API-key flow is granted access outright (master key = owner-level
  // access); a session asks the server.
  const [status, setStatus] = useState<'loading' | 'ok' | 'unauth' | 'forbidden'>(() => (getApiKey() ? 'ok' : 'loading'));

  useEffect(() => {
    if (getApiKey()) return;
    client.auth.me()
      .then(me => {
        const r = me as { user?: { role?: string } };
        const role = r.user?.role;
        if (!role) { setStatus('unauth'); return; }
        const allowed = permissionsFor(role).includes(perm);
        setStatus(allowed ? 'ok' : 'forbidden');
      })
      .catch(() => setStatus('unauth'));
  }, [perm]);

  if (status === 'loading') return <div className="loading">Checking permissions…</div>;
  if (status === 'unauth') return <Navigate to="/login" replace />;
  if (status === 'forbidden') return (
    <div data-testid="forbidden-403" className="alert alert-error" style={{
      padding: '24px 32px',
      background: '#3d1a1a',
      border: '1px solid #f85149',
      borderRadius: 8,
      color: '#f85149',
      margin: 24,
    }}>
      Forbidden: you need <code>{perm}</code> to view this page.
    </div>
  );
  return <>{children}</>;
}
