import type { ReactNode } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { useTenant } from '../context/tenant';

/**
 * Gate that mirrors the storefront admin router guard: unauthenticated visitors
 * are redirected to a clean /login (no app shell). While the session check is in
 * flight we render a minimal centered loading state — never the sidebar.
 */
export default function RequireAuth({ children }: { children: ReactNode }) {
  const { authenticated, loading } = useTenant();
  const location = useLocation();

  if (authenticated === null || loading) {
    return (
      <div
        data-testid="auth-loading"
        style={{
          display: 'flex',
          justifyContent: 'center',
          alignItems: 'center',
          minHeight: '100vh',
          background: '#0d1117',
          color: '#8b949e',
          fontSize: 14,
        }}
      >
        <span>Loading…</span>
      </div>
    );
  }

  if (authenticated === false) {
    const next = location.pathname + location.search;
    return <Navigate to={`/login?next=${encodeURIComponent(next)}`} replace />;
  }

  return <>{children}</>;
}
