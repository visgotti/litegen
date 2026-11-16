// What the sign-in pages read from `GET /v1/auth/config`, kept free of React
// so it is unit-tested.
import type { AuthConfigResponse } from '@litegen/sdk';

/** The config as a page holds it: on a failed read, the fallback below, which
 *  knows no password length (the server's 400 says it). */
export type SignInConfig = Omit<AuthConfigResponse, 'password_min_length'> & { password_min_length?: number };

/** Shown when the config can't be read: the password form and both providers. */
export const SIGN_IN_FALLBACK: SignInConfig = { password_enabled: true, providers_enabled: ['github', 'google'], signup_open: false };

/** The password field's hint, from the minimum the server serves; nothing while it hasn't said. */
export function passwordHint(config: SignInConfig | null): string {
  const n = config?.password_min_length;
  return typeof n === 'number' && n > 0 ? `at least ${n} characters` : '';
}
