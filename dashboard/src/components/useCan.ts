import { getApiKey } from '../sdk-client';
import { useTenant } from '../context/tenant';
import { offerFor } from './permissions';

/**
 * Whether the signed-in principal holds a permission in the active org, so a
 * page offers only what it can use; the server enforces every permission. A
 * session holds what `/v1/auth/me` serves for the active org (the server's
 * own rule); the API-key login is the master key, which may do anything.
 */
export function useCan(): (perm: string) => boolean {
  const { orgs, activeOrg } = useTenant();
  return offerFor({ master: Boolean(getApiKey()), orgs, activeOrg });
}
