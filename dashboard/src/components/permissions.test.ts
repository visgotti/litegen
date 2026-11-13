import { describe, expect, it } from 'vitest';
import type { MeOrg } from '@litegen/sdk';
import { offerFor, servedPermissions } from './permissions';

// What a session is offered is what `GET /v1/auth/me` serves for the org it
// acts in (FU-ME): each org's `permissions`, by the rule the server's checks
// use (the role there; the deployment's own six only in a single-tenant
// deployment's default org, never in hosted mode). The lists are the binary's
// own answers (F3's probe, a single-tenant first signup and a second org of
// its; hosted, the same). The server still enforces every permission.

/** An Owner's list in any org but a single-tenant deployment's default one (38). */
const OWNER_ANYWHERE = [
  'app:delete', 'app:read', 'app:write', 'audit:read', 'generation:cancel:any', 'generation:cancel:own',
  'generation:create', 'generation:delete:any', 'generation:delete:own', 'generation:read:any',
  'generation:read:own', 'invitation:revoke', 'invitation:send', 'key:delete:any', 'key:delete:own',
  'key:read:any', 'key:read:own', 'key:test_webhook:any', 'key:test_webhook:own', 'key:write:any',
  'key:write:own', 'member:invite', 'member:read', 'member:remove', 'member:write', 'org:delete', 'org:read',
  'org:transfer_owner', 'org:write', 'provider_cred:delete', 'provider_cred:read', 'provider_cred:write',
  'session:revoke:any', 'session:revoke:own', 'storage_cred:delete', 'storage_cred:read',
  'storage_cred:write', 'user:read:self',
];
/** The deployment's own permissions, served only for a single-tenant deployment's default org. */
const DEPLOYMENT_OWN = ['cache:clear', 'system:config', 'system:transfer_owner', 'user:delete:any', 'user:read:any', 'user:write:any'];
const DEFAULT_ORG = '00000000-0000-0000-0000-000000000001';

const org = (id: string, role: string, permissions: string[]): MeOrg => ({ id, name: id, role, permissions });

describe('the permissions served for the active org', () => {
  const orgs = [org(DEFAULT_ORG, 'owner', [...OWNER_ANYWHERE, ...DEPLOYMENT_OWN]), org('side', 'owner', OWNER_ANYWHERE)];

  it("are that org's own list: a single-tenant Owner holds the deployment's permissions only in its default org", () => {
    expect([...servedPermissions(orgs, DEFAULT_ORG)].sort()).toEqual([...OWNER_ANYWHERE, ...DEPLOYMENT_OWN].sort());
    expect(servedPermissions(orgs, 'side')).toEqual(OWNER_ANYWHERE);
    for (const p of DEPLOYMENT_OWN) expect(servedPermissions(orgs, 'side'), p).not.toContain(p);
  });

  it("hold none of the deployment's own for a hosted Owner", () => {
    const can = offerFor({ master: false, orgs: [org('acme', 'owner', OWNER_ANYWHERE)], activeOrg: 'acme' });
    for (const p of DEPLOYMENT_OWN) expect(can(p), p).toBe(false);
    expect(can('org:write')).toBe(true);
    expect(can('key:write:any')).toBe(true);
  });

  it('decide, whatever the role says', () => {
    const viewerWhoGenerates = offerFor({ master: false, orgs: [org('a', 'viewer', ['generation:create'])], activeOrg: 'a' });
    expect(viewerWhoGenerates('generation:create')).toBe(true);
    const ownerWithNothing = offerFor({ master: false, orgs: [org('a', 'owner', [])], activeOrg: 'a' });
    expect(ownerWithNothing('org:write')).toBe(false);
  });

  it('are none with no active org, or one the person is not in', () => {
    expect(servedPermissions(orgs, null)).toEqual([]);
    expect(servedPermissions(orgs, 'an-org-it-left')).toEqual([]);
    expect(offerFor({ master: false, orgs: [], activeOrg: null })('user:read:self')).toBe(false);
  });
});

describe('the master key', () => {
  it('(the API-key login) is offered everything, in any org or none, as before', () => {
    const can = offerFor({ master: true, orgs: [], activeOrg: null });
    for (const p of ['system:config', 'key:write:any', 'org:write', 'generation:create']) expect(can(p), p).toBe(true);
  });
});
