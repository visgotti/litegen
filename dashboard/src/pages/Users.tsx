import { useState, useEffect, useCallback } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import { showToast } from '../components/toast-store';

interface PublicUser {
  id: string;
  email: string;
  role: string;
  is_active: boolean;
  last_login_at?: string | null;
  created_at: string;
}

interface CurrentUser {
  role: string;
  id?: string;
}

const ROLE_OPTIONS = ['admin', 'member', 'viewer'] as const;

function RoleBadge({ role }: { role: string }) {
  const colors: Record<string, { bg: string; color: string }> = {
    owner: { bg: '#31213a', color: '#d2a8ff' },
    admin: { bg: '#1a3f5c', color: '#58a6ff' },
    member: { bg: '#1a4731', color: '#3fb950' },
    viewer: { bg: '#2d2a1a', color: '#e3b341' },
  };
  const c = colors[role] ?? { bg: '#21262d', color: '#8b949e' };
  return (
    <span style={{
      fontSize: 12,
      fontWeight: 600,
      padding: '2px 10px',
      borderRadius: 999,
      background: c.bg,
      color: c.color,
      textTransform: 'capitalize',
    }}>
      {role}
    </span>
  );
}

export default function Users() {
  const [users, setUsers] = useState<PublicUser[]>([]);
  const [loading, setLoading] = useState(true);
  const [me, setMe] = useState<CurrentUser | null>(null);

  // Invite modal
  const [showInviteModal, setShowInviteModal] = useState(false);
  const [inviteEmail, setInviteEmail] = useState('');
  const [inviteRole, setInviteRole] = useState<string>('member');
  const [inviteLoading, setInviteLoading] = useState(false);
  const [inviteDevToken, setInviteDevToken] = useState<string | null>(null);

  // Edit role inline
  const [editingUserId, setEditingUserId] = useState<string | null>(null);
  const [editRole, setEditRole] = useState('');
  const [editLoading, setEditLoading] = useState(false);

  // Transfer ownership confirm
  const [transferTargetId, setTransferTargetId] = useState<string | null>(null);

  const fetchUsers = useCallback(async () => {
    setLoading(true);
    try {
      const list = await client.users.list();
      setUsers(list as PublicUser[]);
    } catch {
      // handled via RequirePermission or onError
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchMe = useCallback(async () => {
    try {
      const meResp = await client.auth.me() as { user?: { role: string; id?: string } };
      if (meResp?.user) {
        setMe(meResp.user);
      }
    } catch {
      // no session
    }
  }, []);

  useEffect(() => {
    fetchMe();
    fetchUsers();
  }, [fetchMe, fetchUsers]);

  const handleInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    setInviteLoading(true);
    setInviteDevToken(null);
    try {
      const resp = await client.users.invite({ email: inviteEmail, role: inviteRole as 'owner' | 'admin' | 'member' | 'viewer' });
      const respAny = resp as { _dev_token?: string; id?: string };
      if (respAny._dev_token) {
        setInviteDevToken(respAny._dev_token);
      } else {
        showToast('Invitation sent', 'info');
        setShowInviteModal(false);
        setInviteEmail('');
      }
      await fetchUsers();
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        showToast(err.message ?? 'Failed to send invitation', 'error');
      }
    } finally {
      setInviteLoading(false);
    }
  };

  const handleEditRole = async (userId: string) => {
    setEditLoading(true);
    try {
      await client.users.patch(userId, { role: editRole as 'owner' | 'admin' | 'member' | 'viewer' });
      setEditingUserId(null);
      await fetchUsers();
      showToast('Role updated', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        showToast(err.message ?? 'Failed to update role', 'error');
      }
    } finally {
      setEditLoading(false);
    }
  };

  const handleSetActive = async (userId: string, active: boolean) => {
    try {
      await client.users.patch(userId, { is_active: active });
      await fetchUsers();
      showToast(active ? 'User reactivated' : 'User deactivated', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        showToast(err.message ?? 'Failed to update user', 'error');
      }
    }
  };

  const handleTransferOwner = async () => {
    if (!transferTargetId) return;
    try {
      await client.users.transferOwner({ new_owner_id: transferTargetId });
      setTransferTargetId(null);
      await fetchUsers();
      await fetchMe();
      showToast('Ownership transferred', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        showToast(err.message ?? 'Transfer failed', 'error');
      }
    }
  };

  const isOwner = me?.role === 'owner';

  return (
    <div style={{ padding: '0 0 32px' }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 24 }}>
        <h2 style={{ margin: 0, color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Users</h2>
        <button
          className="btn btn-primary"
          data-testid="users-invite-btn"
          onClick={() => { setShowInviteModal(true); setInviteDevToken(null); }}
        >
          + Invite user
        </button>
      </div>

      {/* Invite Modal */}
      {showInviteModal && (
        <div style={{
          position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.7)', zIndex: 1000,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }} onClick={e => { if (e.target === e.currentTarget) setShowInviteModal(false); }}>
          <div style={{ width: 400, background: '#161b22', border: '1px solid #30363d', borderRadius: 12, padding: 28 }}>
            <h3 style={{ margin: '0 0 20px', color: '#e6edf3', fontSize: 18 }}>Invite user</h3>

            {inviteDevToken ? (
              <div>
                <div style={{ padding: '12px 16px', background: '#1a4731', border: '1px solid #3fb950', borderRadius: 8, marginBottom: 16 }}>
                  <div style={{ color: '#3fb950', fontSize: 13, fontWeight: 600, marginBottom: 6 }}>Invitation sent!</div>
                  <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 4 }}>Dev token (share invitation link):</div>
                  <code
                    data-testid="invite-dev-token"
                    style={{ display: 'block', color: '#e6edf3', fontSize: 12, wordBreak: 'break-all', background: '#0d1117', padding: '8px 10px', borderRadius: 6 }}
                  >
                    {inviteDevToken}
                  </code>
                </div>
                <button className="btn btn-secondary" data-testid="invite-dev-token-close" onClick={() => { setShowInviteModal(false); setInviteDevToken(null); setInviteEmail(''); }}>
                  Close
                </button>
              </div>
            ) : (
              <form onSubmit={handleInvite} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
                <div>
                  <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Email</label>
                  <input
                    className="input"
                    data-testid="invite-email"
                    type="email"
                    value={inviteEmail}
                    onChange={e => setInviteEmail(e.target.value)}
                    placeholder="user@example.com"
                    required
                    style={{ width: '100%', boxSizing: 'border-box' }}
                  />
                </div>
                <div>
                  <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Role</label>
                  <select
                    className="input"
                    data-testid="invite-role"
                    value={inviteRole}
                    onChange={e => setInviteRole(e.target.value)}
                    style={{ width: '100%', boxSizing: 'border-box' }}
                  >
                    {ROLE_OPTIONS.map(r => (
                      <option key={r} value={r}>{r.charAt(0).toUpperCase() + r.slice(1)}</option>
                    ))}
                  </select>
                </div>
                <div style={{ display: 'flex', gap: 10, marginTop: 4 }}>
                  <button className="btn btn-primary" data-testid="invite-send" type="submit" disabled={inviteLoading}>
                    {inviteLoading ? 'Sending…' : 'Send invitation'}
                  </button>
                  <button className="btn btn-secondary" type="button" onClick={() => setShowInviteModal(false)}>
                    Cancel
                  </button>
                </div>
              </form>
            )}
          </div>
        </div>
      )}

      {/* Transfer confirmation modal */}
      {transferTargetId && (
        <div style={{
          position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.7)', zIndex: 1000,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }}>
          <div style={{ width: 400, background: '#161b22', border: '1px solid #30363d', borderRadius: 12, padding: 28 }}>
            <h3 style={{ margin: '0 0 12px', color: '#e6edf3', fontSize: 18 }}>Transfer ownership</h3>
            <p style={{ color: '#8b949e', fontSize: 14, marginBottom: 20 }}>
              You will lose owner status. This action cannot be undone without admin access.
            </p>
            <div style={{ display: 'flex', gap: 10 }}>
              <button className="btn btn-primary" data-testid="confirm-transfer" onClick={handleTransferOwner}
                style={{ background: '#da3633', borderColor: '#da3633' }}>
                Transfer ownership
              </button>
              <button className="btn btn-secondary" onClick={() => setTransferTargetId(null)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {loading ? (
        <div style={{ color: '#8b949e', padding: 24 }}>Loading users…</div>
      ) : (
        <div data-testid="users-table" style={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 10, overflow: 'hidden' }}>
          <table style={{ width: '100%', borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ borderBottom: '1px solid #30363d' }}>
                {['Email', 'Role', 'Last login', 'Status', 'Actions'].map(h => (
                  <th key={h} style={{ padding: '12px 16px', textAlign: 'left', color: '#8b949e', fontSize: 12, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {users.map(u => (
                <tr key={u.id} data-testid={`user-row-${u.email}`} style={{ borderBottom: '1px solid #21262d' }}>
                  <td style={{ padding: '12px 16px', color: '#e6edf3', fontSize: 14 }}>{u.email}</td>
                  <td style={{ padding: '12px 16px' }}>
                    {editingUserId === u.id ? (
                      <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                        <select
                          className="input"
                          data-testid="user-edit-role"
                          value={editRole}
                          onChange={e => setEditRole(e.target.value)}
                          style={{ fontSize: 13, padding: '4px 8px' }}
                        >
                          {ROLE_OPTIONS.map(r => (
                            <option key={r} value={r}>{r.charAt(0).toUpperCase() + r.slice(1)}</option>
                          ))}
                        </select>
                        <button
                          className="btn btn-primary"
                          data-testid="user-edit-save"
                          onClick={() => handleEditRole(u.id)}
                          disabled={editLoading}
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          {editLoading ? '…' : 'Save'}
                        </button>
                        <button
                          className="btn btn-secondary"
                          onClick={() => setEditingUserId(null)}
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          ✕
                        </button>
                      </div>
                    ) : (
                      <span data-testid={`user-role-${u.email}`}>
                        <RoleBadge role={u.role} />
                      </span>
                    )}
                  </td>
                  <td style={{ padding: '12px 16px', color: '#8b949e', fontSize: 13 }}>
                    {u.last_login_at ? new Date(u.last_login_at).toLocaleDateString() : 'Never'}
                  </td>
                  <td style={{ padding: '12px 16px' }}>
                    <span style={{
                      fontSize: 12, fontWeight: 600, padding: '2px 8px', borderRadius: 999,
                      background: u.is_active ? '#1a4731' : '#3d1a1a',
                      color: u.is_active ? '#3fb950' : '#f85149',
                    }}>
                      {u.is_active ? 'Active' : 'Inactive'}
                    </span>
                  </td>
                  <td style={{ padding: '12px 16px' }}>
                    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                      {u.role !== 'owner' && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`user-edit-${u.email}`}
                          onClick={() => { setEditingUserId(u.id); setEditRole(u.role); }}
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          Edit role
                        </button>
                      )}
                      {u.is_active && u.role !== 'owner' ? (
                        <button
                          className="btn btn-secondary"
                          data-testid={`user-deactivate-${u.email}`}
                          onClick={() => handleSetActive(u.id, false)}
                          style={{ fontSize: 12, padding: '4px 10px', color: '#f85149', borderColor: '#f85149' }}
                        >
                          Deactivate
                        </button>
                      ) : !u.is_active ? (
                        <button
                          className="btn btn-secondary"
                          onClick={() => handleSetActive(u.id, true)}
                          style={{ fontSize: 12, padding: '4px 10px', color: '#3fb950', borderColor: '#3fb950' }}
                        >
                          Reactivate
                        </button>
                      ) : null}
                      {isOwner && u.role === 'admin' && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`user-transfer-${u.email}`}
                          onClick={() => setTransferTargetId(u.id)}
                          style={{ fontSize: 12, padding: '4px 10px', color: '#d2a8ff', borderColor: '#d2a8ff' }}
                        >
                          Transfer owner
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {users.length === 0 && (
            <div style={{ padding: 32, textAlign: 'center', color: '#8b949e' }}>No users found.</div>
          )}
        </div>
      )}
    </div>
  );
}
