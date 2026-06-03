import { createContext, useContext } from 'react';
import type { Application } from '@litegen/sdk';

export const ACTIVE_ORG_KEY = 'litegen_active_org';
export const ACTIVE_APP_KEY = 'litegen_active_app';

export interface TenantOrg {
  id: string;
  name: string;
  role: string;
}

export interface TenantContextValue {
  /** Orgs the current user belongs to. */
  orgs: TenantOrg[];
  /** Active org id (or null). */
  activeOrg: string | null;
  /** Active app id (or null). */
  activeApp: string | null;
  /** Apps belonging to the active org. */
  apps: Application[];
  /** The current user's role within the active org. */
  activeOrgRole: string | null;
  /** True until the first me() resolves. */
  loading: boolean;
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
