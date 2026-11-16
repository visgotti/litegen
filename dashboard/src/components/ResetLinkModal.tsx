import OneTimeLinkPanel from './OneTimeLinkPanel';

/**
 * A password reset link an admin just made for someone, shown once. litegen
 * sends no email: the admin hands the link to that person only, since whoever
 * opens it sets the account's password. Only Done closes it: the link can't be
 * shown again (make another to replace a lost one).
 */
export default function ResetLinkModal({ email, link, expiresAt, onClose }: {
  email: string;
  link: string;
  expiresAt: string;
  onClose: () => void;
}) {
  return (
    <div style={{
      position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.7)', zIndex: 1000,
      display: 'flex', alignItems: 'center', justifyContent: 'center',
    }}>
      <div data-testid="reset-modal" style={{ width: 440, background: '#161b22', border: '1px solid #30363d', borderRadius: 12, padding: 28 }}>
        <h3 style={{ margin: '0 0 20px', color: '#e6edf3', fontSize: 18 }}>Password reset link</h3>
        <OneTimeLinkPanel
          tid="reset"
          heading={`A new password for ${email}`}
          note={`No email is sent: give this link to ${email} only, since whoever opens it sets the account's password. It works once, until ${new Date(expiresAt).toLocaleString()}, and is not shown again.`}
          link={link}
          onClose={onClose}
        />
      </div>
    </div>
  );
}
