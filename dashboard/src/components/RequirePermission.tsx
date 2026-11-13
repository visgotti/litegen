import type { ReactNode } from 'react';
import { useCan } from './useCan';

interface Props {
  perm: string;
  children: ReactNode;
}

/**
 * A page only a holder of `perm` can use. The API-key flow is the master key,
 * granted outright; a session holds what `/v1/auth/me` serves for its active
 * org (RequireAuth has already resolved it), so switching orgs re-decides.
 */
export default function RequirePermission({ perm, children }: Props) {
  const can = useCan();
  if (!can(perm)) return (
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
