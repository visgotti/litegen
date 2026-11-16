import { useCallback, useEffect, useState } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import type { MemberView } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import InviteLinkPanel from '../components/InviteLinkPanel';
import PendingInvitations from '../components/PendingInvitations';
import ResetLinkModal from '../components/ResetLinkModal';
import { resetLinkOf } from '../components/password-reset';
import { inviteLinkOf } from '../components/invitations';
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
  const { activeOrg, activePermissions, refresh } = useTenant();

  const [members, setMembers] = useState<MemberView[]>([]);
  const [loading, setLoading] = useState(true);

  // Invite modal
  const [showInviteModal, setShowInviteModal] = useState(false);
  const [inviteEmail, setInviteEmail] = useState('');
  const [inviteRole, setInviteRole] = useState<string>('member');
  const [inviteLoading, setInviteLoading] = useState(false);
  // The invitation just made: its link is shown once, to copy (no email is sent).
  const [invited, setInvited] = useState<{ email: string; link: string; expiresAt: string } | null>(null);
  // Bumped after an invite, so the pending list loads again.
  const [invitationsVersion, setInvitationsVersion] = useState(0);
  // A password reset link just made for a member: shown once, to hand over.
  const [resetLink, setResetLink] = useState<{ email: string; link: string; expiresAt: string } | null>(null);

  // Inline role edit: userId being edited
  const [editingUserId, setEditingUserId] = useState<string | null>(null);
  const [editRole, setEditRole] = useState('');

  // Each control is offered on the permission it takes in the active org, as
  // /v1/auth/me serves it (the master key's login holds none here, as before).
  const canInvite = activePermissions.includes('member:invite');
  const canRevokeInvitations = activePermissions.includes('invitation:revoke');
  const canEditRoles = activePermissions.includes('member:write');
  const canRemove = activePermissions.includes('member:remove');
  const canTransfer = activePermissions.includes('org:transfer_owner');

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
    setInvited(null);
    try {
      const resp = await client.orgs.members.invite(activeOrg, {
        email: inviteEmail,
        role: inviteRole as Role,
      });
      setInvited({ email: resp.email, link: inviteLinkOf(resp, window.location.origin), expiresAt: resp.expires_at });
      setInviteEmail('');
      setInvitationsVersion(v => v + 1);
      await fetchMembers();
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Failed to create the invitation', 'error');
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

  // Who may make one is the server's to say (an Owner or Admin who outranks
  // the member in every org it belongs to): a refusal is toasted.
  const handleResetPassword = async (userId: string) => {
    try {
      const r = await client.users.passwordResetLink(userId);
      setResetLink({ email: r.email, link: resetLinkOf(r, window.location.origin), expiresAt: r.expires_at });
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Could not make a reset link', 'error');
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
        {canInvite && (
          <button
            className="btn btn-primary"
            data-testid="members-invite-btn"
            onClick={() => { setShowInviteModal(true); setInvited(null); }}
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
        }} onClick={e => {
          // A link on show is not shown again: only Done closes it.
          if (e.target === e.currentTarget && !invited) setShowInviteModal(false);
        }}>
          <div data-testid="invite-modal" style={{ width: 440, background: '#161b22', border: '1px solid #30363d', borderRadius: 12, padding: 28 }}>
            <h3 style={{ margin: '0 0 20px', color: '#e6edf3', fontSize: 18 }}>Invite member</h3>

            {invited ? (
              <InviteLinkPanel email={invited.email} link={invited.link} expiresAt={invited.expiresAt}
                onClose={() => { setShowInviteModal(false); setInvited(null); }} />
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
                    {inviteLoading ? 'Creating…' : 'Create invitation link'}
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
                      {canEditRoles && m.role !== 'owner' && editingUserId !== m.user_id && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`member-edit-${m.email}`}
                          onClick={() => { setEditingUserId(m.user_id); setEditRole(m.role); }}
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          Edit role
                        </button>
                      )}
                      {canTransfer && m.role !== 'owner' && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`member-transfer-${m.email}`}
                          onClick={() => handleTransfer(m.user_id)}
                          style={{ fontSize: 12, padding: '4px 10px', color: '#d2a8ff', borderColor: '#d2a8ff' }}
                        >
                          Transfer owner
                        </button>
                      )}
                      {canEditRoles && m.role !== 'owner' && (
                        <button
                          className="btn btn-secondary"
                          data-testid={`member-reset-password-${m.email}`}
                          onClick={() => handleResetPassword(m.user_id)}
                          title="Make a link that sets a new password for this member"
                          style={{ fontSize: 12, padding: '4px 10px' }}
                        >
                          Reset password
                        </button>
                      )}
                      {canRemove && m.role !== 'owner' && (
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

      {resetLink && (
        <ResetLinkModal email={resetLink.email} link={resetLink.link} expiresAt={resetLink.expiresAt} onClose={() => setResetLink(null)} />
      )}

      {/* Listing takes member:invite, as the server checks it. */}
      {canInvite && <PendingInvitations orgId={activeOrg} canRevoke={canRevokeInvitations} version={invitationsVersion} />}
    </div>
  );
}
