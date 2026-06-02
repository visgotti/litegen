import { type ReactNode, useEffect, useState } from 'react';
import { Navigate } from 'react-router-dom';
import { client, getApiKey } from '../sdk-client';

interface Props {
  perm: string;
  children: ReactNode;
}

export default function RequirePermission({ perm, children }: Props) {
  const [status, setStatus] = useState<'loading' | 'ok' | 'unauth' | 'forbidden'>('loading');

  useEffect(() => {
    // If using the API-key flow, grant access (master key = owner-level access).
    if (getApiKey()) {
      setStatus('ok');
      return;
    }
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

function permissionsFor(role: string): string[] {
  // Mirror the backend's role → permission map. Owner > Admin > Member > Viewer.
  if (role === 'owner') return [
    'user:read:self', 'user:read:any', 'user:write:any', 'user:delete:any',
    'key:read:own', 'key:read:any', 'key:write:own', 'key:write:any',
    'key:delete:own', 'key:delete:any', 'key:test_webhook:own', 'key:test_webhook:any',
    'generation:create', 'generation:read:own', 'generation:read:any',
    'generation:cancel:own', 'generation:cancel:any',
    'audit:read', 'cache:clear', 'system:config', 'system:transfer_owner',
    'invitation:send', 'invitation:revoke',
    'session:revoke:own', 'session:revoke:any',
  ];
  if (role === 'admin') return [
    'user:read:self', 'user:read:any', 'user:write:any', 'user:delete:any',
    'key:read:own', 'key:read:any', 'key:write:own', 'key:write:any',
    'key:delete:own', 'key:delete:any', 'key:test_webhook:own', 'key:test_webhook:any',
    'generation:create', 'generation:read:own', 'generation:read:any',
    'generation:cancel:own', 'generation:cancel:any',
    'audit:read', 'cache:clear', 'system:config',
    'invitation:send', 'invitation:revoke',
    'session:revoke:own', 'session:revoke:any',
  ];
  if (role === 'member') return [
    'user:read:self', 'key:read:own', 'key:write:own', 'key:delete:own', 'key:test_webhook:own',
    'generation:create', 'generation:read:own', 'generation:cancel:own', 'session:revoke:own',
  ];
  if (role === 'viewer') return [
    'user:read:self', 'key:read:own', 'generation:create', 'generation:read:own', 'session:revoke:own',
  ];
  return [];
}
