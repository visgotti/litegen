// The OAuth consent page's logic, kept free of React so it is unit-tested on
// its own: what each scope means, which orgs the person may approve in, what
// starts ticked, and what the approval sends.
import { LiteGenAPIError, type ApproveOAuthRequest, type OAuthRequestView } from '@litegen/sdk';

type OrgChoice = OAuthRequestView['orgs'][number];

/** What the person has picked on the page. */
export interface ConsentChoice {
  orgId: string;
  appId: string | null;
  scopes: string[];
}

/** One requested scope, as the page lists it for an org. */
export interface ScopeRow {
  scope: string;
  description: string;
  grantable: boolean;
}

const SCOPE_DESCRIPTIONS: Record<string, string> = {
  'ai:generate': "Generate images, video and 3D models (spends this org's budget)",
  'ai:read': 'See models and your generations',
  'admin:read': 'See keys, budgets, spend, logs and members',
  'admin:write': 'Change keys, budgets, end users, members and settings',
};

export const EXPIRED_MESSAGE = 'This sign-in request has expired. Go back to the app and connect again.';

export function describeScope(scope: string): string {
  return SCOPE_DESCRIPTIONS[scope] ?? scope;
}

export function consentTitle(view: OAuthRequestView): string {
  const what = view.surface === 'admin' ? 'LiteGen admin (management API)' : 'LiteGen';
  return `${view.client.name} wants to use ${what}`;
}

/** The orgs where the person's role can grant at least one requested scope. */
export function selectableOrgs(view: OAuthRequestView): OrgChoice[] {
  return view.orgs.filter(org => org.grantable_scopes.length > 0);
}

/** Every requested scope, marked with whether the person's role in `orgId` can grant it. */
export function scopeRows(view: OAuthRequestView, orgId: string): ScopeRow[] {
  const org = view.orgs.find(o => o.id === orgId);
  return view.requested_scopes.map(scope => ({
    scope,
    description: describeScope(scope),
    grantable: org?.grantable_scopes.includes(scope) ?? false,
  }));
}

/** What starts picked in `orgId`: its first app and every requested scope the person can grant there. */
export function choiceForOrg(view: OAuthRequestView, orgId: string): ConsentChoice {
  const org = view.orgs.find(o => o.id === orgId);
  return {
    orgId,
    appId: org?.apps[0]?.id ?? null,
    scopes: view.requested_scopes.filter(s => org?.grantable_scopes.includes(s)),
  };
}

/** The first org where the person can grant something, as {@link choiceForOrg} picks it. */
export function initialChoice(view: OAuthRequestView): ConsentChoice | null {
  const org = selectableOrgs(view)[0];
  return org ? choiceForOrg(view, org.id) : null;
}

/** `choice` with `scope` ticked or unticked, in the order the app asked for its scopes. */
export function toggleScope(view: OAuthRequestView, choice: ConsentChoice, scope: string, on: boolean): ConsentChoice {
  const ticked = (s: string) => (s === scope ? on : choice.scopes.includes(s));
  return { ...choice, scopes: view.requested_scopes.filter(ticked) };
}

export function approveBody(choice: ConsentChoice): ApproveOAuthRequest {
  return { org_id: choice.orgId, ...(choice.appId ? { app_id: choice.appId } : {}), scopes: choice.scopes };
}

/** What to tell the person when loading or deciding failed. */
export function consentError(err: unknown): string {
  if (err instanceof LiteGenAPIError && (err.status === 404 || err.status === 409)) return EXPIRED_MESSAGE;
  return err instanceof Error ? err.message : String(err);
}
