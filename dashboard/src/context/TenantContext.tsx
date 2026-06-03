import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { client, getApiKey } from '../sdk-client';
import {
  ACTIVE_ORG_KEY,
  ACTIVE_APP_KEY,
  TenantContext,
  type TenantContextValue,
  type TenantOrg,
} from './tenant';
import type { Application } from '@litegen/sdk';

interface MeResponse {
  user?: { id?: string; email?: string; role?: string };
  orgs?: TenantOrg[];
  active_org?: string | null;
  active_app?: string | null;
}

export function TenantProvider({ children }: { children: ReactNode }) {
  const [orgs, setOrgs] = useState<TenantOrg[]>([]);
  const [activeOrg, setActiveOrg] = useState<string | null>(
    () => localStorage.getItem(ACTIVE_ORG_KEY),
  );
  const [activeApp, setActiveApp] = useState<string | null>(
    () => localStorage.getItem(ACTIVE_APP_KEY),
  );
  const [apps, setApps] = useState<Application[]>([]);
  const [loading, setLoading] = useState(true);
  // Track which org's apps are loaded (also guards StrictMode double-mount in dev).
  const loadedAppsForOrg = useRef<string | null>(null);

  const persistOrg = useCallback((orgId: string | null) => {
    if (orgId) localStorage.setItem(ACTIVE_ORG_KEY, orgId);
    else localStorage.removeItem(ACTIVE_ORG_KEY);
  }, []);

  const persistApp = useCallback((appId: string | null) => {
    if (appId) localStorage.setItem(ACTIVE_APP_KEY, appId);
    else localStorage.removeItem(ACTIVE_APP_KEY);
  }, []);

  // Load an org's apps; default the active app to the first if none/invalid selected.
  const loadApps = useCallback(async (orgId: string, desiredAppId?: string | null): Promise<void> => {
    try {
      const list = await client.orgs.apps.list(orgId);
      setApps(list);
      loadedAppsForOrg.current = orgId;
      const valid = desiredAppId && list.some(a => a.id === desiredAppId)
        ? desiredAppId
        : (list[0]?.id ?? null);
      setActiveApp(valid);
      persistApp(valid);
      client.setActiveTenant(orgId, valid ?? undefined);
    } catch {
      setApps([]);
    }
  }, [persistApp]);

  const refresh = useCallback(async (): Promise<void> => {
    // API-key flow has no org/app session context — skip tenant resolution.
    if (getApiKey()) {
      setLoading(false);
      return;
    }
    try {
      const resp = (await client.auth.me()) as MeResponse;
      const fetchedOrgs = resp.orgs ?? [];
      setOrgs(fetchedOrgs);

      const storedOrg = localStorage.getItem(ACTIVE_ORG_KEY);
      const storedApp = localStorage.getItem(ACTIVE_APP_KEY);
      // Prefer stored org if still valid, else the server's active_org, else first org.
      const orgId =
        (storedOrg && fetchedOrgs.some(o => o.id === storedOrg) && storedOrg) ||
        (resp.active_org && fetchedOrgs.some(o => o.id === resp.active_org) && resp.active_org) ||
        fetchedOrgs[0]?.id ||
        null;

      setActiveOrg(orgId);
      persistOrg(orgId);

      if (orgId) {
        const desiredApp = resp.active_app ?? storedApp ?? null;
        await loadApps(orgId, desiredApp);
      } else {
        setApps([]);
        setActiveApp(null);
        persistApp(null);
        client.setActiveTenant(undefined, undefined);
      }
    } catch {
      // Not authenticated (401) or backend not in hosted mode — leave tenant unset.
      setOrgs([]);
    } finally {
      setLoading(false);
    }
  }, [loadApps, persistOrg, persistApp]);

  useEffect(() => {
    void refresh();
    // Re-resolve tenant context when the user re-authenticates elsewhere.
    const onUnauth = () => {
      setOrgs([]);
      setApps([]);
    };
    window.addEventListener('litegen:unauthenticated', onUnauth);
    return () => window.removeEventListener('litegen:unauthenticated', onUnauth);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const switchOrg = useCallback(async (orgId: string): Promise<void> => {
    setActiveOrg(orgId);
    persistOrg(orgId);
    // Reset app selection; loadApps will default to the org's first app.
    setActiveApp(null);
    client.setActiveTenant(orgId, undefined);
    await loadApps(orgId, null);
  }, [loadApps, persistOrg]);

  const switchApp = useCallback((appId: string): void => {
    setActiveApp(appId);
    persistApp(appId);
    client.setActiveTenant(activeOrg ?? undefined, appId);
  }, [activeOrg, persistApp]);

  const activeOrgRole = useMemo(() => {
    if (!activeOrg) return null;
    return orgs.find(o => o.id === activeOrg)?.role ?? null;
  }, [orgs, activeOrg]);

  const value = useMemo<TenantContextValue>(() => ({
    orgs,
    activeOrg,
    activeApp,
    apps,
    activeOrgRole,
    loading,
    switchOrg,
    switchApp,
    refresh,
  }), [orgs, activeOrg, activeApp, apps, activeOrgRole, loading, switchOrg, switchApp, refresh]);

  return <TenantContext.Provider value={value}>{children}</TenantContext.Provider>;
}
