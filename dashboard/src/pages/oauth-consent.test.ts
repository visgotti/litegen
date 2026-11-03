import { describe, it, expect } from 'vitest';
import { LiteGenAPIError, type OAuthRequestView } from '@litegen/sdk';
import {
  approveBody,
  choiceForOrg,
  consentError,
  consentTitle,
  describeScope,
  initialChoice,
  scopeRows,
  selectableOrgs,
  toggleScope,
} from './oauth-consent';

const view = (over: Partial<OAuthRequestView> = {}): OAuthRequestView => ({
  id: 'req-1',
  surface: 'ai',
  requested_scopes: ['ai:generate', 'ai:read'],
  expires_at: '2026-11-03T00:10:00Z',
  client: {
    client_id: 'lgc_x',
    name: 'Claude',
    logo_uri: null,
    client_uri: null,
    redirect_host: 'claude.ai',
    registration: 'cimd',
  },
  orgs: [
    { id: 'o-viewer', name: 'Read-only org', role: 'viewer', apps: [{ id: 'a0', name: 'Default' }], grantable_scopes: [] },
    { id: 'o-member', name: 'Team', role: 'member', apps: [{ id: 'a1', name: 'Web' }, { id: 'a2', name: 'Batch' }], grantable_scopes: ['ai:generate', 'ai:read'] },
    { id: 'o-owner', name: 'Mine', role: 'owner', apps: [{ id: 'a3', name: 'Default' }], grantable_scopes: ['ai:generate', 'ai:read'] },
  ],
  ...over,
});

describe('the OAuth consent page', () => {
  it('says who asks for which endpoint', () => {
    expect(consentTitle(view())).toBe('Claude wants to use LiteGen');
    expect(consentTitle(view({ surface: 'admin' }))).toBe('Claude wants to use LiteGen admin (management API)');
  });

  it('describes each scope in plain words', () => {
    expect(describeScope('ai:generate')).toBe("Generate images, video and 3D models (spends this org's budget)");
    expect(describeScope('ai:read')).toBe('See models and your generations');
    expect(describeScope('admin:read')).toBe('See keys, budgets, spend, logs and members');
    expect(describeScope('admin:write')).toBe('Change keys, budgets, end users, members and settings');
    expect(describeScope('something:new')).toBe('something:new');
  });

  it('offers only the orgs where the person can grant something', () => {
    expect(selectableOrgs(view()).map(o => o.id)).toEqual(['o-member', 'o-owner']);
  });

  it('starts on the first such org, its first app, and every scope it can grant', () => {
    expect(initialChoice(view())).toEqual({ orgId: 'o-member', appId: 'a1', scopes: ['ai:generate', 'ai:read'] });
    const nothing = view({ orgs: [view().orgs[0]] });
    expect(initialChoice(nothing)).toBeNull();
  });

  it('marks the scopes a role cannot grant in an org', () => {
    const admin = view({
      surface: 'admin',
      requested_scopes: ['admin:read', 'admin:write'],
      orgs: [{ id: 'o1', name: 'Team', role: 'member', apps: [], grantable_scopes: ['admin:read'] }],
    });
    expect(scopeRows(admin, 'o1')).toEqual([
      { scope: 'admin:read', description: 'See keys, budgets, spend, logs and members', grantable: true },
      { scope: 'admin:write', description: 'Change keys, budgets, end users, members and settings', grantable: false },
    ]);
    expect(initialChoice(admin)).toEqual({ orgId: 'o1', appId: null, scopes: ['admin:read'] });
  });

  it('picking another org starts over with its first app and what can be granted there', () => {
    expect(choiceForOrg(view(), 'o-owner')).toEqual({ orgId: 'o-owner', appId: 'a3', scopes: ['ai:generate', 'ai:read'] });
    expect(choiceForOrg(view(), 'o-viewer')).toEqual({ orgId: 'o-viewer', appId: 'a0', scopes: [] });
  });

  it('ticks and unticks scopes in the order the app asked for them', () => {
    const start = { orgId: 'o-member', appId: 'a1', scopes: ['ai:generate', 'ai:read'] };
    const unticked = toggleScope(view(), start, 'ai:generate', false);
    expect(unticked.scopes).toEqual(['ai:read']);
    expect(toggleScope(view(), unticked, 'ai:generate', true).scopes).toEqual(['ai:generate', 'ai:read']);
  });

  it('approves with the org, the app when there is one, and the ticked scopes', () => {
    expect(approveBody({ orgId: 'o1', appId: 'a1', scopes: ['ai:read'] })).toEqual({ org_id: 'o1', app_id: 'a1', scopes: ['ai:read'] });
    expect(approveBody({ orgId: 'o1', appId: null, scopes: ['ai:read'] })).toEqual({ org_id: 'o1', scopes: ['ai:read'] });
  });

  it('turns an expired or decided request into the go-back message', () => {
    const apiError = (status: number, code: string, message: string) => new LiteGenAPIError(status, { message, type: 'request_error', code });
    const expired = 'This sign-in request has expired. Go back to the app and connect again.';
    expect(consentError(apiError(404, 'request_not_found', 'gone'))).toBe(expired);
    expect(consentError(apiError(409, 'request_decided', 'decided'))).toBe(expired);
    expect(consentError(apiError(403, 'scope_not_grantable', "Your role in this organization can't grant admin:write."))).toBe(
      "Your role in this organization can't grant admin:write.",
    );
    expect(consentError(new Error('network down'))).toBe('network down');
  });
});
