import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { client } from '../sdk-client';
import { LiteGenAPIError } from '@litegen/sdk';

export default function Signup() {
  const navigate = useNavigate();
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

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
      await client.auth.signup({ email, password });
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

  return (
    <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', minHeight: '100vh', background: '#0d1117' }}>
      <div style={{ width: 380, padding: 32, background: '#161b22', borderRadius: 12, border: '1px solid #30363d' }}>
        <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 24, fontWeight: 600, textAlign: 'center' }}>
          Create your account
        </h2>

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

        <div style={{ marginTop: 20, textAlign: 'center', color: '#8b949e', fontSize: 13 }}>
          Already have an account?{' '}
          <a href="/login" style={{ color: '#58a6ff', textDecoration: 'none' }}>Sign in</a>
        </div>
      </div>
    </div>
  );
}
