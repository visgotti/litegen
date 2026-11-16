import { useEffect, useState } from 'react';
import type { PendingInvitation } from '@litegen/sdk';
import { LiteGenAPIError } from '@litegen/sdk';
import { client } from '../sdk-client';
import { showToast } from './toast-store';

/**
 * An org's invitations not yet accepted or expired, as
 * `GET /v1/orgs/{id}/invitations` serves them (it takes `member:invite`), with
 * a Revoke button where the session holds `invitation:revoke`. A revoked
 * invitation's link opens nothing. `version` changes to load the list again
 * (after an invite).
 */
export default function PendingInvitations({ orgId, canRevoke, version }: { orgId: string; canRevoke: boolean; version: number }) {
  const [invitations, setInvitations] = useState<PendingInvitation[] | null>(null);
  // Bumped after a revoke, so the list loads again.
  const [reloads, setReloads] = useState(0);

  useEffect(() => {
    let cancelled = false;
    client.orgs.invitations.list(orgId)
      .then(list => { if (!cancelled) setInvitations(list); })
      .catch(() => { /* a refusal is toasted by the client */ });
    return () => { cancelled = true; };
  }, [orgId, version, reloads]);

  const revoke = async (inv: PendingInvitation) => {
    try {
      await client.orgs.invitations.revoke(orgId, inv.id);
      showToast(`Invitation for ${inv.email} revoked`, 'info');
      setReloads(r => r + 1);
    } catch (err) {
      if (err instanceof LiteGenAPIError) showToast(err.message ?? 'Revoke failed', 'error');
    }
  };

  if (invitations === null) return null;
  return (
    <div style={{ marginTop: 24 }}>
      <h3 style={{ margin: '0 0 12px', color: '#e6edf3', fontSize: 16 }}>Pending invitations</h3>
      <div data-testid="invitations-table" style={{ background: '#161b22', border: '1px solid #30363d', borderRadius: 10, overflow: 'hidden' }}>
        {invitations.length === 0 ? (
          <div data-testid="invitations-empty" style={{ padding: 24, textAlign: 'center', color: '#8b949e' }}>No pending invitations.</div>
        ) : (
          <table style={{ width: '100%', borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ borderBottom: '1px solid #30363d' }}>
                {['Email', 'Role', 'Invited', 'Expires', 'Actions'].map(h => (
                  <th key={h} style={{ padding: '12px 16px', textAlign: 'left', color: '#8b949e', fontSize: 12, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {invitations.map(inv => (
                <tr key={inv.id} data-testid={`invitation-row-${inv.email}`} style={{ borderBottom: '1px solid #21262d' }}>
                  <td style={{ padding: '12px 16px', color: '#e6edf3', fontSize: 14 }}>{inv.email}</td>
                  <td style={{ padding: '12px 16px', color: '#e6edf3', fontSize: 13, textTransform: 'capitalize' }}>{inv.role}</td>
                  <td style={{ padding: '12px 16px', color: '#8b949e', fontSize: 12 }}>{new Date(inv.created_at).toLocaleString()}</td>
                  <td style={{ padding: '12px 16px', color: '#8b949e', fontSize: 12 }}>{new Date(inv.expires_at).toLocaleString()}</td>
                  <td style={{ padding: '12px 16px' }}>
                    {canRevoke && (
                      <button
                        className="btn btn-secondary"
                        data-testid={`invitation-revoke-${inv.email}`}
                        onClick={() => void revoke(inv)}
                        style={{ fontSize: 12, padding: '4px 10px', color: '#f85149', borderColor: '#f85149' }}
                      >
                        Revoke
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
