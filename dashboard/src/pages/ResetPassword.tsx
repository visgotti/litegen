import { useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { LiteGenAPIError } from '@litegen/sdk';
import { client } from '../sdk-client';

/**
 * A password reset link's page (`/reset-password/:token`): a new password,
 * sent with the link's token to `POST /v1/auth/password-reset/confirm`, then on
 * to sign in. What a password must be, and whether the link still works, is
 * the server's to say, in its words.
 */
export default function ResetPassword() {
  const { token = '' } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');
    // A typo guard, not a rule: the two fields must match.
    if (password !== confirmPassword) {
      setError('The two passwords differ');
      return;
    }
    setLoading(true);
    try {
      await client.auth.passwordResetConfirm({ token, new_password: password });
      // The reset ends every session: the person signs in with the new password.
      navigate('/login?reset=done');
    } catch (err) {
      setError(err instanceof LiteGenAPIError ? (err.message ?? 'Could not set the password') : 'Network error, please try again');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="auth-page" data-testid="auth-page">
      <div className="auth-card">
        <div className="auth-brand"><img className="brand-mark" src="/litegen-logo.png" alt="" /> LiteGen</div>
        <h2 style={{ margin: '0 0 24px', color: '#e6edf3', fontSize: 24, fontWeight: 600, textAlign: 'center' }}>
          Set a new password
        </h2>
        <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>New password</label>
            <input
              className="input"
              data-testid="reset-password"
              type="password"
              autoComplete="new-password"
              value={password}
              onChange={e => setPassword(e.target.value)}
              required
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>
          <div>
            <label style={{ display: 'block', marginBottom: 6, color: '#8b949e', fontSize: 13 }}>Confirm the new password</label>
            <input
              className="input"
              data-testid="reset-confirm-password"
              type="password"
              autoComplete="new-password"
              value={confirmPassword}
              onChange={e => setConfirmPassword(e.target.value)}
              required
              style={{ width: '100%', boxSizing: 'border-box' }}
            />
          </div>
          {error && (
            <div
              data-testid="reset-error"
              style={{ padding: '10px 14px', background: '#3d1a1a', border: '1px solid #f85149', borderRadius: 6, color: '#f85149', fontSize: 13 }}
            >
              {error}
            </div>
          )}
          <button className="btn btn-primary" data-testid="reset-submit" type="submit" disabled={loading} style={{ marginTop: 4 }}>
            {loading ? 'Setting the password…' : 'Set the password'}
          </button>
        </form>
        <div className="auth-switch-link">
          <Link to="/login" data-testid="reset-back">Back to sign in</Link>
        </div>
      </div>
    </div>
  );
}
