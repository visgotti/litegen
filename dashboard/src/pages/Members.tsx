import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { MemberView } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';

const ROLE_OPTIONS = ['admin', 'member', 'viewer'] as const;
type Role = 'owner' | 'admin' | 'member' | 'viewer';

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
      fontSize: 12, fontWeight: 600, padding: '2px 10px', borderRadius: 999,
      background: c.bg, color: c.color, textTransform: 'capitalize',
    }}>
      {role}
    </span>
  );
}

export default function Members() {
  const { activeOrg, activeOrgRole, refresh } = useTenant();

  const [members, setMembers] = useState<MemberView[]>([]);
  const [loading, setLoading] = useState(true);

  // Invite modal
  const [showInviteModal, setShowInviteModal] = useState(false);
  const [inviteEmail, setInviteEmail] = useState('');
  const [inviteRole, setInviteRole] = useState<string>('member');
  const [inviteLoading, setInviteLoading] = useState(false);
  const [inviteDevToken, setInviteDevToken] = useState<string | null>(null);

  // Inline role edit: userId being edited
  const [editingUserId, setEditingUserId] = useState<string | null>(null);
  const [editRole, setEditRole] = useState('');

  // Privileged controls (invite / transfer / role-edit / remove) gate on org role.
  const canManage = activeOrgRole === 'owner' || activeOrgRole === 'admin';
  const isOwner = activeOrgRole === 'owner';

  const fetchMembers = useCallback(async () => {
    if (!activeOrg) { setMembers([]); setLoading(false); return; }
    setLoading(true);
    try {
      const list = await client.orgs.members.list(activeOrg);
      setMembers(list);
    } catch {
      // surfaced via onError toast
    } finally {
      setLoading(false);
    }
  }, [activeOrg]);

  useEffect(() => { void fetchMembers(); }, [fetchMembers]);

  const handleInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!activeOrg) return;
    setInviteLoading(true);
    setInviteDevToken(null);
    try {
      const resp = await client.orgs.members.invite(activeOrg, {
        email: inviteEmail,
        role: inviteRole as Role,
      });
      const respAny = resp as { _dev_token?: string };
      if (respAny._dev_token) {
        setInviteDevToken(respAny._dev_token);
      } else {
        showToast('Invitation sent', 'info');
        setShowInviteModal(false);
        setInviteEmail('');
      }
      await fetchMembers();
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Failed to send invitation', 'error');
    } finally {
      setInviteLoading(false);
    }
  };

  const handleEditRole = async (userId: string) => {
    if (!activeOrg) return;
    try {
      await client.orgs.members.updateRole(activeOrg, userId, { role: editRole as Role });
      setEditingUserId(null);
      await fetchMembers();
      showToast('Role updated', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Failed to update role', 'error');
    }
  };

  const handleTransfer = async (userId: string) => {
    if (!activeOrg) return;
    try {
      await client.orgs.transferOwner(activeOrg, { new_owner_user_id: userId });
      await fetchMembers();
      await refresh();
      showToast('Ownership transferred', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Transfer failed', 'error');
    }
  };

  const handleRemove = async (userId: string) => {
    if (!activeOrg) return;
    try {
      await client.orgs.members.remove(activeOrg, userId);
      await fetchMembers();
      showToast('Member removed', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Remove failed', 'error');
    }
  };

  if (!activeOrg) {
    return (
      <div style={{ padding: 24, color: '#8b949e' }} data-testid="members-no-active">
        No active organization.
      </div>
    );
  }

  return (
    <div style={{ padding: '0 0 32px' }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 24 }}>
        <h2 style={{ margin: 0, color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Members</h2>
        {canManage && (
          <button
            className="btn btn-primary"
            data-testid="members-invite-btn"
            onClick={() => { setShowInviteModal(true); setInviteDevToken(null); }}
          >
            + Invite member
          </button>
        )}
      </div>

      {/* Invite modal */}
      {showInviteModal && (
        <div style={{
          position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.7)', zIndex: 1000,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }} onClick={e => { if (e.target === e.currentTarget) setShowInviteModal(false); }}>
          <div style={{ width: 400, background: '#161b22', border: '1px solid #30363d', borderRadius: 12, padding: 28 }}>
            <h3 style={{ margin: '0 0 20px', color: '#e6edf3', fontSize: 18 }}>Invite member</h3>

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

      {loading ? (
        <div style={{ color: '#8b949e', padding: 24 }}>Loading members…</div>
      ) : (
        <div data-testid="members-table" style={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 10, overflow: 'hidden' }}>
          <table style={{ width: '100%', borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ borderBottom: '1px solid #30363d' }}>
                {['Email', 'Role', 'Actions'].map(h => (
                  <th key={h} style={{ padding: '12px 16px', textAlign: 'left', color: '#8b949e', fontSize: 12, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {members.map(m => (
                <tr key={m.user_id} data-testid={`member-row-${m.email}`} style={{ borderBottom: '1px solid #21262d' }}>
                  <td style={{ padding: '12px 16px', color: '#e6edf3', fontSize: 14 }}>{m.email}</td>
                  <td style={{ padding: '12px 16px' }}>
                    {editingUserId === m.user_id ? (
                      <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                        <select
                          className="input"
                          data-testid={`member-edit-role-${m.email}`}
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
                          data-testid={`member-edit-save-${m.email}`}
                          onClick={() => handleEditRole(m.user_id)}
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          Save
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
                      <span data-testid={`member-role-${m.email}`}>
                        <RoleBadge role={m.role} />
                      </span>
                    )}
                  </td>
                  <td style={{ padding: '12px 16px' }}>
                    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                      {canManage && m.role !== 'owner' && editingUserId !== m.user_id && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`member-edit-${m.email}`}
                          onClick={() => { setEditingUserId(m.user_id); setEditRole(m.role); }}
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          Edit role
                        </button>
                      )}
                      {isOwner && m.role !== 'owner' && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`member-transfer-${m.email}`}
                          onClick={() => handleTransfer(m.user_id)}
                          style={{ fontSize: 12, padding: '4px 10px', color: '#d2a8ff', borderColor: '#d2a8ff' }}
                        >
                          Transfer owner
                        </button>
                      )}
                      {canManage && m.role !== 'owner' && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`member-remove-${m.email}`}
                          onClick={() => handleRemove(m.user_id)}
                          style={{ fontSize: 12, padding: '4px 10px', color: '#f85149', borderColor: '#f85149' }}
                        >
                          Remove
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {members.length === 0 && (
            <div style={{ padding: 32, textAlign: 'center', color: '#8b949e' }}>No members found.</div>
          )}
        </div>
      )}
    </div>
  );
}
