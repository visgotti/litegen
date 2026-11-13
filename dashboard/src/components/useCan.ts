import { getApiKey } from '../sdk-client';
import { useTenant } from '../context/tenant';
import { roleCan } from './permissions';

/**
 * Whether the signed-in principal holds a permission in the active org, so a
 * page offers only what it can use; the server enforces every permission. A
 * session acts by its role in the active org (as the server's does); the
 * API-key login is the master key, which may do anything.
 */
export function useCan(): (perm: string) => boolean {
  const { activeOrgRole } = useTenant();
  const master = Boolean(getApiKey());
  return perm => master || roleCan(activeOrgRole, perm);
}
