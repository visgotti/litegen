import { createContext, useContext } from 'react';
import type { Application, MeOrg } from '@litegen/sdk';

export const ACTIVE_ORG_KEY = 'litegen_active_org';
export const ACTIVE_APP_KEY = 'litegen_active_app';

export interface TenantContextValue {
  /** Orgs the current user belongs to, as `/v1/auth/me` serves them: role and permissions in each. */
  orgs: MeOrg[];
  /** Active org id (or null). */
  activeOrg: string | null;
  /** Active app id (or null). */
  activeApp: string | null;
  /** Apps belonging to the active org. */
  apps: Application[];
  /** The current user's role within the active org (for display: what it may do is `activePermissions`). */
  activeOrgRole: string | null;
  /**
   * The permissions `/v1/auth/me` serves for the active org: what a session
   * is offered there. Empty with no active org, and for the API-key login
   * (`useCan` offers the master key everything).
   */
  activePermissions: readonly string[];
  /** True until the first me() resolves. */
  loading: boolean;
  /**
   * Auth state derived from the session:
   *  - null  → still checking (initial me() not yet resolved)
   *  - true  → me() succeeded (or the API-key flow is active)
   *  - false → 401 / unauthenticated
   */
  authenticated: boolean | null;
  switchOrg: (orgId: string) => Promise<void>;
  switchApp: (appId: string) => void;
  refresh: () => Promise<void>;
}

export const TenantContext = createContext<TenantContextValue | null>(null);

export function useTenant(): TenantContextValue {
  const ctx = useContext(TenantContext);
  if (!ctx) throw new Error('useTenant must be used within a TenantProvider');
  return ctx;
}
