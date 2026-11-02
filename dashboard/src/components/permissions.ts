// The backend's role → permission map, for showing only what a role can do.
// The server still enforces every permission.
export function permissionsFor(role: string): string[] {
  // Mirror the backend's role → permission map. Owner > Admin > Member > Viewer.
  if (role === 'owner') return [
    'user:read:self', 'user:read:any', 'user:write:any', 'user:delete:any',
    'key:read:own', 'key:read:any', 'key:write:own', 'key:write:any',
    'key:delete:own', 'key:delete:any', 'key:test_webhook:own', 'key:test_webhook:any',
    'generation:create', 'generation:read:own', 'generation:read:any',
    'generation:cancel:own', 'generation:cancel:any',
    'audit:read', 'cache:clear', 'system:config', 'system:transfer_owner',
    'invitation:send', 'invitation:revoke',
    'session:revoke:own', 'session:revoke:any',
  ];
  if (role === 'admin') return [
    'user:read:self', 'user:read:any', 'user:write:any', 'user:delete:any',
    'key:read:own', 'key:read:any', 'key:write:own', 'key:write:any',
    'key:delete:own', 'key:delete:any', 'key:test_webhook:own', 'key:test_webhook:any',
    'generation:create', 'generation:read:own', 'generation:read:any',
    'generation:cancel:own', 'generation:cancel:any',
    'audit:read', 'cache:clear', 'system:config',
    'invitation:send', 'invitation:revoke',
    'session:revoke:own', 'session:revoke:any',
  ];
  if (role === 'member') return [
    'user:read:self', 'key:read:own', 'key:write:own', 'key:delete:own', 'key:test_webhook:own',
    'generation:create', 'generation:read:own', 'generation:cancel:own', 'session:revoke:own',
  ];
  if (role === 'viewer') return [
    'user:read:self', 'key:read:own', 'generation:create', 'generation:read:own', 'session:revoke:own',
  ];
  return [];
}
