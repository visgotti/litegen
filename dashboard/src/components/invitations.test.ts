import { describe, expect, it } from 'vitest';
import { inviteLinkOf } from './invitations';

// No email is sent for an invitation: the inviter hands its link over. Inside
// the dashboard the link is always to this dashboard's own accept page
// (`/invite/:token`, App.tsx) on the origin the inviter is using: the
// server's `invite_url` is built from `server.public_base_url`, which can be
// another host than this dashboard, and is there for API callers.

describe('inviteLinkOf', () => {
  it("is this dashboard's accept page, whatever link the server made", () => {
    expect(inviteLinkOf({ token: 'tok_abc', invite_url: 'https://api.example/invite/tok_abc' }, 'http://127.0.0.1:5174'))
      .toBe('http://127.0.0.1:5174/invite/tok_abc');
    expect(inviteLinkOf({ token: 'tok_abc', invite_url: null }, 'http://127.0.0.1:5174'))
      .toBe('http://127.0.0.1:5174/invite/tok_abc');
  });

  it('carries the token safely in the path', () => {
    expect(inviteLinkOf({ token: 'a/b c', invite_url: null }, 'https://dash.example'))
      .toBe('https://dash.example/invite/a%2Fb%20c');
  });
});
