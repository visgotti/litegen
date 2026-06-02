import { useState, useEffect, useCallback } from 'react';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';
import { showToast } from '../components/toast-store';

interface AccountUser {
  id: string;
  email: string;
  role: string;
  created_at: string;
  is_active: boolean;
  last_login_at?: string | null;
}

interface SessionInfo {
  id: string;
  created_at: string;
  expires_at: string;
  ip?: string | null;
  user_agent?: string | null;
}

export default function Account() {
  const [user, setUser] = useState<AccountUser | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [loadingUser, setLoadingUser] = useState(true);
  const [loadingSessions, setLoadingSessions] = useState(true);

  // Change password form
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmNewPassword, setConfirmNewPassword] = useState('');
  const [pwError, setPwError] = useState('');
  const [pwLoading, setPwLoading] = useState(false);

  // Revoking sessions
  const [revokingId, setRevokingId] = useState<string | null>(null);

  const fetchUser = useCallback(async () => {
    try {
      const u = await client.account.get();
      setUser(u as AccountUser);
    } catch {
      // handled silently
    } finally {
      setLoadingUser(false);
    }
  }, []);

  const fetchSessions = useCallback(async () => {
    setLoadingSessions(true);
    try {
      const list = await client.account.listSessions();
      setSessions(list as SessionInfo[]);
    } catch {
      // handled silently
    } finally {
      setLoadingSessions(false);
    }
  }, []);

  useEffect(() => {
    fetchUser();
    fetchSessions();
  }, [fetchUser, fetchSessions]);

  const handleChangePassword = async (e: React.FormEvent) => {
    e.preventDefault();
    setPwError('');

    if (newPassword.length < 12) {
      setPwError('New password must be at least 12 characters');
      return;
    }
    if (newPassword !== confirmNewPassword) {
      setPwError('Passwords do not match');
      return;
    }

    setPwLoading(true);
    try {
      await client.account.patch({ current_password: currentPassword, new_password: newPassword });
      setCurrentPassword('');
      setNewPassword('');
      setConfirmNewPassword('');
      showToast('Password updated', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        setPwError(err.message ?? 'Failed to change password');
      } else {
        setPwError('Network error, please try again');
      }
    } finally {
      setPwLoading(false);
    }
  };

  const handleRevokeSession = async (sessionId: string) => {
    setRevokingId(sessionId);
    try {
      await client.account.revokeSession(sessionId);
      await fetchSessions();
      showToast('Session revoked', 'info');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        showToast(err.message ?? 'Failed to revoke session', 'error');
      }
    } finally {
      setRevokingId(null);
    }
  };

  const roleBadgeStyle = (role: string) => {
    const colors: Record<string, { bg: string; color: string }> = {
      owner: { bg: '#31213a', color: '#d2a8ff' },
      admin: { bg: '#1a3f5c', color: '#58a6ff' },
      member: { bg: '#1a4731', color: '#3fb950' },
      viewer: { bg: '#2d2a1a', color: '#e3b341' },
    };
    const c = colors[role] ?? { bg: '#21262d', color: '#8b949e' };
    return {
      fontSize: 12,
      fontWeight: 600 as const,
      padding: '2px 10px',
      borderRadius: 999,
      background: c.bg,
      color: c.color,
      textTransform: 'capitalize' as const,
    };
  };

  if (loadingUser) {
    return <div style={{ padding: 32, color: '#e6edf3' }}>Loading…</div>;
  }

  return (
    <div style={{ maxWidth: 640, padding: '32px 0' }}>
      <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 22, fontWeight: 600 }}>Account</h2>

      {/* Profile section */}
      <div style={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 10, padding: 24, marginBottom: 24 }}>
        <h3 style={{ margin: '0 0 16px', color: '#e6edf3', fontSize: 16, fontWeight: 600 }}>Profile</h3>
        <div style={{ display: 'flex', gap: 16, alignItems: 'center' }}>
          <div>
            <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 4 }}>Email</div>
            <div data-testid="account-email" style={{ color: '#e6edf3', fontWeight: 500 }}>{user?.email}</div>
          </div>
          <div style={{ marginLeft: 'auto' }}>
            <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 4 }}>Role</div>
            <span data-testid="account-role" style={roleBadgeStyle(user?.role ?? '')}>{user?.role}</span>
          </div>
        </div>
      </div>

      {/* Change password section */}
      <div style={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 10, padding: 24, marginBottom: 24 }}>
        <h3 style={{ margin: '0 0 16px', color: '#e6edf3', fontSize: 16, fontWeight: 600 }}>Change password</h3>
        <form onSubmit={handleChangePassword} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Current password</label>
            <input
              className="input"
              data-testid="account-current-password"
              type="password"
              value={currentPassword}
              onChange={e => setCurrentPassword(e.target.value)}
              placeholder="••••••••••••"
              required
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>
          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>New password</label>
            <input
              className="input"
              data-testid="account-new-password"
              type="password"
              value={newPassword}
              onChange={e => setNewPassword(e.target.value)}
              placeholder="••••••••••••"
              required
              minLength={12}
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>
          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Confirm new password</label>
            <input
              className="input"
              type="password"
              value={confirmNewPassword}
              onChange={e => setConfirmNewPassword(e.target.value)}
              placeholder="••••••••••••"
              required
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>

          {pwError && (
            <div style={{ padding: '8px 12px', background: '#3d1a1a', border: '1px solid #f85149', borderRadius: 6, color: '#f85149', fontSize: 13 }}>
              {pwError}
            </div>
          )}

          <button
            className="btn btn-primary"
            data-testid="account-change-password-submit"
            type="submit"
            disabled={pwLoading}
            style={{ alignSelf: 'flex-start' }}
          >
            {pwLoading ? 'Updating…' : 'Update password'}
          </button>
        </form>
      </div>

      {/* Sessions section */}
      <div style={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 10, padding: 24 }}>
        <h3 style={{ margin: '0 0 16px', color: '#e6edf3', fontSize: 16, fontWeight: 600 }}>Active sessions</h3>

        {loadingSessions ? (
          <div style={{ color: '#8b949e', fontSize: 13 }}>Loading sessions…</div>
        ) : sessions.length === 0 ? (
          <div style={{ color: '#8b949e', fontSize: 13 }}>No active sessions found.</div>
        ) : (
          <div data-testid="account-sessions-table" style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {sessions.map(sess => (
              <div
                key={sess.id}
                data-testid={`account-session-row-${sess.id}`}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  padding: '12px 16px',
                  background: '#0d1117',
                  borderRadius: 8,
                  border: '1px solid #30363d',
                  gap: 12,
                }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ color: '#e6edf3', fontSize: 13, fontWeight: 500, marginBottom: 2 }}>
                    {sess.user_agent ? sess.user_agent.substring(0, 60) + (sess.user_agent.length > 60 ? '…' : '') : 'Unknown browser'}
                  </div>
                  <div style={{ color: '#6e7681', fontSize: 12 }}>
                    {sess.ip ?? 'Unknown IP'} · Created {new Date(sess.created_at).toLocaleDateString()}
                  </div>
                </div>
                <button
                  className="btn btn-secondary"
                  data-testid={`account-revoke-session-${sess.id}`}
                  onClick={() => handleRevokeSession(sess.id)}
                  disabled={revokingId === sess.id}
                  style={{ fontSize: 12, padding: '4px 12px', whiteSpace: 'nowrap', flexShrink: 0 }}
                >
                  {revokingId === sess.id ? 'Revoking…' : 'Revoke'}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
