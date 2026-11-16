import { Link } from 'react-router-dom';

/**
 * Where "Forgot password?" leads. litegen sends no email, so there is no form
 * to fill in here: an admin of the person's organization makes a reset link
 * and hands it over, and the link opens this dashboard's reset page.
 */
export default function ForgotPassword() {
  return (
    <div className="auth-page" data-testid="auth-page">
      <div className="auth-card" data-testid="forgot-password">
        <div className="auth-brand"><img className="brand-mark" src="/litegen-logo.png" alt="" /> LiteGen</div>
        <h2 style={{ margin: '0 0 16px', color: '#e6edf3', fontSize: 24, fontWeight: 600, textAlign: 'center' }}>
          Forgot your password?
        </h2>
        <p style={{ margin: '0 0 12px', color: '#8b949e', fontSize: 14, lineHeight: 1.5 }}>
          No email is sent: litegen has no mail service. Ask an admin of your organization for a password reset
          link. They make one where they manage its people, and it lets you set a new password once, until it
          expires.
        </p>
        <p style={{ margin: 0, color: '#8b949e', fontSize: 14, lineHeight: 1.5 }}>
          If you signed in with GitHub or Google, sign in with it again instead.
        </p>
        <div className="auth-switch-link">
          <Link to="/login" data-testid="forgot-back">Back to sign in</Link>
        </div>
      </div>
    </div>
  );
}
