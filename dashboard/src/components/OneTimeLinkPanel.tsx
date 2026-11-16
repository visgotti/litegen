import type { ReactNode } from 'react';
import { showToast } from './toast-store';

/**
 * A link the server shows only once (an invitation's, a password reset's),
 * with a Copy button and Done. litegen sends no email, so the person on this
 * page hands the link over. Test ids: `${tid}-link`, `${tid}-link-copy`,
 * `${tid}-close`.
 */
export default function OneTimeLinkPanel({ tid, heading, note, link, onClose }: {
  tid: string;
  heading: string;
  note: ReactNode;
  link: string;
  onClose: () => void;
}) {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(link);
      showToast('Link copied', 'info');
    } catch {
      showToast('Failed to copy', 'error');
    }
  };
  return (
    <div>
      <div style={{ padding: '12px 16px', background: '#1a4731', border: '1px solid #3fb950', borderRadius: 8, marginBottom: 16 }}>
        <div style={{ color: '#3fb950', fontSize: 13, fontWeight: 600, marginBottom: 6 }}>{heading}</div>
        <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 8 }}>{note}</div>
        <div style={{ display: 'flex', gap: 8 }}>
          <input
            className="input"
            readOnly
            data-testid={`${tid}-link`}
            value={link}
            onFocus={e => e.target.select()}
            style={{ flex: 1, minWidth: 0, fontSize: 12 }}
          />
          <button type="button" className="btn btn-secondary" data-testid={`${tid}-link-copy`} onClick={() => void copy()}>
            Copy
          </button>
        </div>
      </div>
      <button type="button" className="btn btn-secondary" data-testid={`${tid}-close`} onClick={onClose}>
        Done
      </button>
    </div>
  );
}
