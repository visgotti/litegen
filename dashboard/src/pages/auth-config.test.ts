import { describe, expect, it } from 'vitest';
import { SIGN_IN_FALLBACK, passwordHint } from './auth-config';

// A password's rules are the server's: GET /v1/auth/config serves its
// minimum length (`password_min_length`), which the sign-up, invitation and
// account forms show; the server's 400 says why it refuses one.

describe('passwordHint', () => {
  it('says the minimum the server serves', () => {
    expect(passwordHint({ ...SIGN_IN_FALLBACK, password_min_length: 12 })).toBe('at least 12 characters');
    expect(passwordHint({ ...SIGN_IN_FALLBACK, password_min_length: 16 })).toBe('at least 16 characters');
  });

  it('says nothing while the server has not said', () => {
    expect(passwordHint(null)).toBe('');
    expect(passwordHint(SIGN_IN_FALLBACK)).toBe('');
  });
});
