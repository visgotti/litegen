import { useState } from 'react';
import { Link } from 'react-router-dom';
import { LiteGenAPIError } from '@litegen/sdk';
import { quietClient } from '../sdk-client';
import { useTenant } from '../context/tenant';

/**
 * For a signed-in person who belongs to no organization (removed from their
 * only one, or its org deleted). Such a person acts nowhere, so this page
 * offers what still works: making an organization (whether they may is the
 * server's to say, and its refusal is shown here), joining one through an
 * invitation link, and their own account.
 */
export default function NoOrganization() {
  const { refresh } = useTenant();
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError('');
    try {
      await quietClient.orgs.create({ name });
      // The new org is listed now: the dashboard opens in it.
      await refresh();
    } catch (err) {
      setError(err instanceof LiteGenAPIError ? err.message : 'Network error, please try again');
      setBusy(false);
    }
  };

  return (
    <div data-testid="no-org" style={{ maxWidth: 560 }}>
      <h2 className="page-title">You are not in an organization</h2>
      <p className="mg-hint" style={{ margin: '0 0 20px' }}>
        Your account belongs to no organization, so there is nothing to read or change here yet. Make an organization
        of your own, or join one through an invitation.
      </p>

      <form onSubmit={create} style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 12 }}>
        <input
          className="input"
          data-testid="no-org-name"
          aria-label="Organization name"
          placeholder="Organization name"
          value={name}
          onChange={e => setName(e.target.value)}
          style={{ flex: 1 }}
        />
        <button className="btn btn-primary" data-testid="no-org-create" type="submit" disabled={busy}>
          Create organization
        </button>
      </form>
      {error && <div className="alert alert-error" data-testid="no-org-error">{error}</div>}

      <p className="mg-hint" data-testid="no-org-invitation" style={{ margin: '20px 0 8px' }}>
        To join an organization that exists, ask it for an invitation and open the link: you join with this account.
      </p>
      <p className="mg-hint" style={{ margin: 0 }}>
        Your password and sign-ins are under <Link to="/account" style={{ color: '#58a6ff' }}>Account</Link>.
      </p>
    </div>
  );
}
