import type { MeOrg } from '@litegen/sdk';

// What the signed-in principal is offered, from what the server serves: `GET
// /v1/auth/me` lists each org with the permissions held acting there, by the
// rule the server's own checks use (the role in that org; the deployment's
// own permissions only in a single-tenant deployment's default org). The page
// offers only what that list holds; the server still enforces every
// permission, and its refusal is what counts.

/** The permissions `/v1/auth/me` serves for `activeOrg`: none when no org is active or the person is not in it. */
export function servedPermissions(orgs: readonly MeOrg[], activeOrg: string | null): readonly string[] {
  return orgs.find(o => o.id === activeOrg)?.permissions ?? [];
}

/**
 * What the principal is offered in the active org: everything for the master
 * key (the API-key login), else what the active org's served list holds.
 */
export function offerFor(p: { master: boolean; orgs: readonly MeOrg[]; activeOrg: string | null }): (perm: string) => boolean {
  if (p.master) return () => true;
  const held = servedPermissions(p.orgs, p.activeOrg);
  return perm => held.includes(perm);
}
