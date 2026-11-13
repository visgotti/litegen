// The backend's role → permission map (litegen-core auth/permissions.rs), for
// offering only what a role can use: the server still enforces every
// permission, and its refusal is what counts. README's roles table is the
// contract: a Member manages their own keys, creates generations and reads;
// a Viewer reads; an app's settings, limits, model access, storage and access,
// and the org's vendor keys are an Owner's or an Admin's.
// /v1/auth/me serves the role, not the permissions, so this copy stays until
// it does.

/** Per-organization permissions every member holds. */
const ORG_READS = ['org:read', 'app:read', 'member:read', 'provider_cred:read', 'storage_cred:read'];

/** The per-organization permissions an Admin holds; an Owner also deletes the org and hands it over. */
const ORG_ADMIN = [
  ...ORG_READS,
  'org:write', 'app:write', 'app:delete', 'member:invite', 'member:write', 'member:remove',
  'provider_cred:write', 'provider_cred:delete', 'storage_cred:write', 'storage_cred:delete',
];

const ADMIN = [
  'user:read:self', 'user:read:any', 'user:write:any', 'user:delete:any',
  'key:read:own', 'key:read:any', 'key:write:own', 'key:write:any',
  'key:delete:own', 'key:delete:any', 'key:test_webhook:own', 'key:test_webhook:any',
  'generation:create', 'generation:read:own', 'generation:read:any',
  'generation:cancel:own', 'generation:cancel:any',
  'generation:delete:own', 'generation:delete:any',
  'audit:read', 'cache:clear', 'system:config',
  'invitation:send', 'invitation:revoke',
  'session:revoke:own', 'session:revoke:any',
  ...ORG_ADMIN,
];

export function permissionsFor(role: string): string[] {
  // Mirror the backend's role → permission map. Owner > Admin > Member > Viewer.
  if (role === 'owner') return [...ADMIN, 'system:transfer_owner', 'org:delete', 'org:transfer_owner'];
  if (role === 'admin') return ADMIN;
  if (role === 'member') return [
    'user:read:self', 'key:read:own', 'key:write:own', 'key:delete:own', 'key:test_webhook:own',
    'generation:create', 'generation:read:own', 'generation:cancel:own', 'generation:delete:own', 'session:revoke:own',
    ...ORG_READS,
  ];
  if (role === 'viewer') return ['user:read:self', 'key:read:own', 'generation:read:own', 'session:revoke:own', ...ORG_READS];
  return [];
}

/** Whether `role` holds `perm`; no role holds nothing. */
export function roleCan(role: string | null | undefined, perm: string): boolean {
  return permissionsFor(role ?? '').includes(perm);
}
