import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { client, API_BASE } from '../sdk-client';
import { LiteGenAPIError, type AuthConfigResponse } from '@litegen/sdk';

function oauthStart(provider: 'github' | 'google') {
  window.location.href = `${API_BASE}/v1/auth/oauth/${provider}/start?next=${encodeURIComponent('/')}`;
}

const providerBtnStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  gap: 8,
  padding: '10px 16px',
  background: '#21262d',
  border: '1px solid #30363d',
  borderRadius: 6,
  color: '#e6edf3',
  textDecoration: 'none',
  fontSize: 14,
  fontWeight: 500,
  cursor: 'pointer',
  width: '100%',
  boxSizing: 'border-box',
};

function GithubButton() {
  return (
    <button type="button" data-testid="oauth-github" onClick={() => oauthStart('github')} style={providerBtnStyle}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
        <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z" />
      </svg>
      Sign up with GitHub
    </button>
  );
}

function GoogleButton() {
  return (
    <button type="button" data-testid="oauth-google" onClick={() => oauthStart('google')} style={providerBtnStyle}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none">
        <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4"/>
        <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/>
        <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l3.66-2.84z" fill="#FBBC05"/>
        <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/>
      </svg>
      Sign up with Google
    </button>
  );
}

export default function Signup() {
  const navigate = useNavigate();
  const [email, setEmail] = useState('');
  const [orgName, setOrgName] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [authConfig, setAuthConfig] = useState<AuthConfigResponse | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.auth
      .config()
      .then(cfg => { if (!cancelled) setAuthConfig(cfg); })
      .catch(() => {
        if (!cancelled) setAuthConfig({ password_enabled: true, providers_enabled: ['github', 'google'], signup_open: false });
      });
    return () => { cancelled = true; };
  }, []);

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
      const trimmedOrg = orgName.trim();
      await client.auth.signup({ email, password, ...(trimmedOrg ? { org_name: trimmedOrg } : {}) });
      navigate('/');
    } catch (err) {
      if (err instanceof LiteGenAPIError) {
        if (err.status === 409) {
          setError('Signup is closed — ask an admin to invite you');
        } else if (err.status === 403) {
          const msg = err.message ?? '';
          const emailMatch = msg.match(/Only (.+?) can claim/);
          setError(emailMatch ? `Only ${emailMatch[1]} can claim ownership` : msg || 'Forbidden');
        } else {
          setError(err.message ?? 'Signup failed');
        }
      } else {
        setError('Network error, please try again');
      }
    } finally {
      setLoading(false);
    }
  };

  // Default to enabled while config loads, so the UI never flashes empty.
  const passwordEnabled = authConfig?.password_enabled ?? true;
  const providers = authConfig?.providers_enabled ?? ['github', 'google'];
  const githubEnabled = providers.includes('github');
  const googleEnabled = providers.includes('google');

  return (
    <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', minHeight: '100vh', background: '#0d1117' }}>
      <div style={{ width: 380, padding: 32, background: '#161b22', borderRadius: 12, border: '1px solid #30363d' }}>
        <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 24, fontWeight: 600, textAlign: 'center' }}>
          Create your account
        </h2>

        {passwordEnabled && (
          <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
            <div>
              <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Email</label>
              <input
                className="input"
                data-testid="signup-email"
                type="email"
                value={email}
                onChange={e => setEmail(e.target.value)}
                placeholder="you@example.com"
                required
                style={{ width: '100%', boxSizing: 'border-box' }}
              />
            </div>

            <div>
              <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>
                Organization name <span style={{ color: '#6e7681' }}>(optional)</span>
              </label>
              <input
                className="input"
                data-testid="signup-org-name"
                type="text"
                value={orgName}
                onChange={e => setOrgName(e.target.value)}
                placeholder="Acme Inc."
                style={{ width: '100%', boxSizing: 'border-box' }}
              />
            </div>

            <div>
              <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>
                Password <span style={{ color: '#6e7681' }}>(min 12 chars)</span>
              </label>
              <input
                className="input"
                data-testid="signup-password"
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
                data-testid="signup-confirm-password"
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
                data-testid="signup-error"
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
              data-testid="signup-submit"
              type="submit"
              disabled={loading}
              style={{ marginTop: 4 }}
            >
              {loading ? 'Creating account…' : 'Create account'}
            </button>
          </form>
        )}

        {(githubEnabled || googleEnabled) && (
          <div style={{ margin: passwordEnabled ? '24px 0 0' : 0, display: 'flex', flexDirection: 'column', gap: 10 }}>
            {passwordEnabled && (
              <div style={{ textAlign: 'center', color: '#8b949e', fontSize: 12, marginBottom: 4 }}>or continue with</div>
            )}
            {githubEnabled && <GithubButton />}
            {googleEnabled && <GoogleButton />}
          </div>
        )}

        {passwordEnabled && (
          <div style={{ marginTop: 20, textAlign: 'center', color: '#8b949e', fontSize: 13 }}>
            Already have an account?{' '}
            <a href="/login" style={{ color: '#58a6ff', textDecoration: 'none' }}>Sign in</a>
          </div>
        )}
      </div>
    </div>
  );
}
