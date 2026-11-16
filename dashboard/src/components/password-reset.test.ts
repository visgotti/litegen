import { describe, expect, it } from 'vitest';
import { resetLinkOf } from './password-reset';

// No email is sent for a password reset either: an admin hands the link
// over. Inside the dashboard it is always this dashboard's reset page
// (`/reset-password/:token`, App.tsx) on the origin the admin is using; the
// server's `reset_url`, built from its public URL, is for API callers.

describe('resetLinkOf', () => {
  it("is this dashboard's reset page for the token, whatever link the server made", () => {
    expect(resetLinkOf({ token: 'tok_abc', reset_url: 'https://api.example/reset-password/tok_abc' }, 'http://127.0.0.1:5174'))
      .toBe('http://127.0.0.1:5174/reset-password/tok_abc');
    expect(resetLinkOf({ token: 'a/b c', reset_url: null }, 'https://dash.example'))
      .toBe('https://dash.example/reset-password/a%2Fb%20c');
  });
});
