import { useState, useEffect } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { client, API_BASE } from '../sdk-client';
import { LiteGenAPIError, type AuthConfigResponse } from '@litegen/sdk';

function oauthAccept(provider: 'github' | 'google', token: string) {
  window.location.href =
    `${API_BASE}/v1/auth/oauth/${provider}/start?invite=${encodeURIComponent(token)}&next=${encodeURIComponent('/')}`;
}

const INVITE_ERROR_MESSAGES: Record<string, string> = {
  email_mismatch: "That account’s email does not match this invitation. Sign in with the invited email.",
  invitation_invalid: 'This invitation is no longer valid (already used or expired).',
  account_inactive: 'This account is inactive. Contact an administrator.',
};

interface InvitationView {
  email: string;
  role: string;
  expires_at: string;
}

export default function AcceptInvite() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const [invitation, setInvitation] = useState<InvitationView | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [loadingInvite, setLoadingInvite] = useState(true);

  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [authConfig, setAuthConfig] = useState<AuthConfigResponse | null>(null);
  const inviteErrorCode = new URLSearchParams(window.location.search).get('invite_error') ?? '';

  useEffect(() => {
    let cancelled = false;
    client.auth.config()
      .then(cfg => { if (!cancelled) setAuthConfig(cfg); })
      .catch(() => {
        if (!cancelled) setAuthConfig({ password_enabled: true, providers_enabled: ['github', 'google'], signup_open: false });
      });
    return () => { cancelled = true; };
  }, []);

  const providers = authConfig?.providers_enabled ?? ['github', 'google'];
  const passwordEnabled = authConfig?.password_enabled ?? true;

  useEffect(() => {
    if (!token) {
      setNotFound(true);
      setLoadingInvite(false);
      return;
    }
    client.auth.getInvitation(token)
      .then(inv => {
        setInvitation(inv);
      })
      .catch(() => {
        setNotFound(true);
      })
      .finally(() => {
        setLoadingInvite(false);
      });
  }, [token]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');

    if (password.length < 12) {
      setError('Password must be at least 12 characters');
      return;
    }
    if (password !== confirmPassword) {
      setError('Passwords do not match');
      return;
    }

    setLoading(true);
    try {
      await client.auth.acceptInvitation(token!, { password });
      navigate('/');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        setError(err.message ?? 'Failed to accept invitation');
      } else {
        setError('Network error, please try again');
      }
    } finally {
      setLoading(false);
    }
  };

  if (loadingInvite) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', minHeight: '100vh', background: '#0d1117', color: '#e6edf3' }}>
        Loading invitation…
      </div>
    );
  }

  if (notFound) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', minHeight: '100vh', background: '#0d1117' }}>
        <div style={{ textAlign: 'center', color: '#f85149', padding: 32 }}>
          <h2 style={{ color: '#e6edf3', marginBottom: 12 }}>Invitation not found</h2>
          <p>This invitation link is invalid or has expired.</p>
          <a href="/login" style={{ color: '#58a6ff', textDecoration: 'none' }}>Back to sign in</a>
        </div>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', minHeight: '100vh', background: '#0d1117' }}>
      <div style={{ width: 400, padding: 32, background: '#161b22', borderRadius: 12, border: '1px solid #30363d' }}>
        <h2 style={{ margin: '0 0 8px', color: '#e6edf3', fontSize: 24, fontWeight: 600, textAlign: 'center' }}>
          Accept invitation
        </h2>
        <p style={{ margin: '0 0 24px', color: '#8b949e', fontSize: 13, textAlign: 'center' }}>
          {passwordEnabled ? 'Continue with a provider, or set a password' : 'Continue with your provider to join'}
        </p>

        {invitation && (
          <div style={{ marginBottom: 24, padding: '12px 16px', background: '#0d1117', borderRadius: 8, border: '1px solid #30363d' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
              <span style={{ color: '#8b949e', fontSize: 13 }}>Email</span>
              <span data-testid="accept-email" style={{ color: '#e6edf3', fontSize: 13, fontWeight: 500 }}>{invitation.email}</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: '#8b949e', fontSize: 13 }}>Role</span>
              <span
                data-testid="accept-role"
                style={{
                  fontSize: 12,
                  fontWeight: 600,
                  padding: '2px 8px',
                  borderRadius: 999,
                  background: '#1a4731',
                  color: '#3fb950',
                  textTransform: 'capitalize',
                }}
              >
                {invitation.role}
              </span>
            </div>
          </div>
        )}

        {inviteErrorCode && (
          <div data-testid="invite-error" style={{ marginBottom: 16, padding: '10px 14px', background: '#3d1a1a', border: '1px solid #f85149', borderRadius: 6, color: '#f85149', fontSize: 13 }}>
            {INVITE_ERROR_MESSAGES[inviteErrorCode] ?? 'Could not accept this invitation.'}
          </div>
        )}

        {(providers.includes('google') || providers.includes('github')) && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginBottom: passwordEnabled ? 20 : 0 }}>
            {providers.includes('google') && (
              <button type="button" data-testid="accept-oauth-google" className="btn" onClick={() => oauthAccept('google', token!)}>
                Continue with Google
              </button>
            )}
            {providers.includes('github') && (
              <button type="button" data-testid="accept-oauth-github" className="btn" onClick={() => oauthAccept('github', token!)}>
                Continue with GitHub
              </button>
            )}
          </div>
        )}

        {passwordEnabled && (
          <>
        <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>
              Password <span style={{ color: '#6e7681' }}>(min 12 chars)</span>
            </label>
            <input
              className="input"
              data-testid="accept-password"
              type="password"
              value={password}
              onChange={e => setPassword(e.target.value)}
              placeholder="••••••••••••"
              required
              minLength={12}
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>

          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Confirm password</label>
            <input
              className="input"
              data-testid="accept-confirm-password"
              type="password"
              value={confirmPassword}
              onChange={e => setConfirmPassword(e.target.value)}
              placeholder="••••••••••••"
              required
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>

          {error && (
            <div
              data-testid="accept-error"
              style={{
                padding: '10px 14px',
                background: '#3d1a1a',
                border: '1px solid #f85149',
                borderRadius: 6,
                color: '#f85149',
                fontSize: 13,
              }}
            >
              {error}
            </div>
          )}

          <button
            className="btn btn-primary"
            data-testid="accept-submit"
            type="submit"
            disabled={loading}
            style={{ marginTop: 4 }}
          >
            {loading ? 'Setting up account…' : 'Set password & join'}
          </button>
        </form>
          </>
        )}
      </div>
    </div>
  );
}
