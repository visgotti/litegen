import { describe, expect, it } from 'vitest';
import { permissionsFor, roleCan } from './permissions';

// The page offers only what a role can use; the server (auth/permissions.rs)
// enforces every permission. README's roles table is the contract (security,
// 2026-11-12): a Member keeps own keys, generations and reads; a Viewer reads.

describe('what each role is offered', () => {
  it('offers a Member its keys, generations and reads, but no app, storage or vendor-key writes', () => {
    for (const p of ['key:write:own', 'generation:create', 'org:read', 'app:read', 'storage_cred:read', 'provider_cred:read']) {
      expect(roleCan('member', p), p).toBe(true);
    }
    for (const p of ['org:write', 'app:write', 'app:delete', 'member:write', 'provider_cred:write', 'storage_cred:write']) {
      expect(roleCan('member', p), p).toBe(false);
    }
  });

  it('offers a Viewer reads only: no generations, no key writes', () => {
    expect(roleCan('viewer', 'generation:create')).toBe(false);
    expect(roleCan('viewer', 'key:write:own')).toBe(false);
    expect(roleCan('viewer', 'generation:read:own')).toBe(true);
    expect(roleCan('viewer', 'app:read')).toBe(true);
  });

  it('offers an Owner and an Admin the org-scoped writes', () => {
    for (const role of ['owner', 'admin']) {
      for (const p of ['org:write', 'app:write', 'storage_cred:write', 'provider_cred:write', 'member:write']) {
        expect(roleCan(role, p), `${role} ${p}`).toBe(true);
      }
    }
    expect(roleCan('admin', 'org:delete')).toBe(false);
    expect(permissionsFor('owner')).toContain('org:delete');
  });

  it('offers nothing to an unknown or missing role', () => {
    expect(roleCan(null, 'app:read')).toBe(false);
    expect(roleCan('root', 'app:read')).toBe(false);
  });
});
