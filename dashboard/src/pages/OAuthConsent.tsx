import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { OAuthRequestView } from '@litegen/sdk';
import { client } from '../sdk-client';
import {
  EXPIRED_MESSAGE,
  approveBody,
  choiceForOrg,
  consentError,
  consentTitle,
  initialChoice,
  scopeRows,
  selectableOrgs,
  toggleScope,
  type ConsentChoice,
} from './oauth-consent';
import './oauth-consent.css';

/** Who vouches for the app's name: its metadata document's host, or nobody. */
function provenance(view: OAuthRequestView): string {
  if (view.client.registration === 'cimd') {
    try {
      return `Published by ${new URL(view.client.client_id).host}.`;
    } catch {
      return '';
    }
  }
  return "This app registered itself, so LiteGen can't vouch for its name.";
}

/**
 * The MCP OAuth consent page. An app (ChatGPT, Claude, an IDE) sent the
 * person's browser here to ask for access to LiteGen; they pick the org and
 * app it acts in and the scopes it gets, then approve or deny, and go back to
 * the app.
 */
export default function OAuthConsent() {
  const [params] = useSearchParams();
  const requestId = params.get('request') ?? '';
  const [view, setView] = useState<OAuthRequestView | null>(null);
  const [choice, setChoice] = useState<ConsentChoice | null>(null);
  const [error, setError] = useState<string | null>(requestId ? null : EXPIRED_MESSAGE);
  const [busy, setBusy] = useState(false);
  // A page that grants access must never act inside another site's frame.
  const framed = window.top !== window.self;

  useEffect(() => {
    if (!requestId) return;
    let live = true;
    client.oauth
      .getRequest(requestId)
      .then(v => {
        if (!live) return;
        setView(v);
        setChoice(initialChoice(v));
      })
      .catch(e => live && setError(consentError(e)));
    return () => {
      live = false;
    };
  }, [requestId]);

  async function decide(approve: boolean) {
    if (!view) return;
    setBusy(true);
    setError(null);
    try {
      const { redirect_to } =
        approve && choice
          ? await client.oauth.approveRequest(view.id, approveBody(choice))
          : await client.oauth.denyRequest(view.id);
      window.location.assign(redirect_to);
    } catch (e) {
      setError(consentError(e));
      setBusy(false);
    }
  }

  const expired = error === EXPIRED_MESSAGE;
  const orgs = view ? selectableOrgs(view) : [];
  const org = view && choice ? view.orgs.find(o => o.id === choice.orgId) : undefined;

  return (
    <div className="auth-page" data-testid="consent-page">
      <div className="auth-card consent-card">
        <div className="auth-brand"><img className="brand-mark" src="/litegen-logo.png" alt="" /> LiteGen</div>

        {framed ? (
          <div className="consent-notice">For your safety, open this page in its own window to approve an app.</div>
        ) : expired ? (
          <div className="consent-notice" data-testid="consent-expired">{EXPIRED_MESSAGE}</div>
        ) : !view ? (
          error ? (
            <div className="consent-notice error" data-testid="consent-error">{error}</div>
          ) : (
            <div className="consent-sub" data-testid="consent-loading">Loading…</div>
          )
        ) : (
          <>
            <div className="consent-client">
              {view.client.logo_uri && <img className="consent-logo" src={view.client.logo_uri} alt="" referrerPolicy="no-referrer" />}
              <h2 className="consent-title" data-testid="consent-title">{consentTitle(view)}</h2>
            </div>
            <p className="consent-sub">
              {provenance(view)} After you decide, you go back to{' '}
              <strong data-testid="consent-redirect-host">{view.client.redirect_host}</strong>.
            </p>

            {error && <div className="consent-notice error" data-testid="consent-error">{error}</div>}

            {orgs.length === 0 || !choice ? (
              <p className="consent-notice" data-testid="consent-no-org">
                Your role in your organizations can't grant what this app asks for.
              </p>
            ) : (
              <>
                <label className="consent-field">
                  Organization
                  <select
                    className="input"
                    data-testid="consent-org"
                    value={choice.orgId}
                    onChange={e => setChoice(choiceForOrg(view, e.target.value))}
                  >
                    {orgs.map(o => (
                      <option key={o.id} value={o.id}>
                        {o.name} ({o.role})
                      </option>
                    ))}
                  </select>
                </label>

                {org && org.apps.length > 0 && (
                  <label className="consent-field">
                    App it acts in
                    <select
                      className="input"
                      data-testid="consent-app"
                      value={choice.appId ?? ''}
                      onChange={e => setChoice({ ...choice, appId: e.target.value })}
                    >
                      {org.apps.map(a => (
                        <option key={a.id} value={a.id}>{a.name}</option>
                      ))}
                    </select>
                  </label>
                )}

                <ul className="consent-scopes">
                  {scopeRows(view, choice.orgId).map(row => (
                    <li key={row.scope} className={`consent-scope${row.grantable ? '' : ' disabled'}`}>
                      <input
                        id={`scope-${row.scope}`}
                        type="checkbox"
                        data-testid={`consent-scope-${row.scope}`}
                        checked={choice.scopes.includes(row.scope)}
                        disabled={!row.grantable || busy}
                        onChange={e => setChoice(toggleScope(view, choice, row.scope, e.target.checked))}
                      />
                      <label htmlFor={`scope-${row.scope}`}>
                        <span className="consent-scope-desc">{row.description}</span>
                        <span className="consent-scope-code">{row.scope}</span>
                        {!row.grantable && <span className="consent-scope-hint">Your role in this org can't grant this</span>}
                      </label>
                    </li>
                  ))}
                </ul>
              </>
            )}

            <div className="consent-actions">
              <button className="btn btn-secondary" data-testid="consent-deny" disabled={busy} onClick={() => decide(false)}>
                Deny
              </button>
              {choice && orgs.length > 0 && (
                <button
                  className="btn btn-primary"
                  data-testid="consent-approve"
                  disabled={busy || choice.scopes.length === 0}
                  onClick={() => decide(true)}
                >
                  {busy ? 'Working…' : 'Approve'}
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
