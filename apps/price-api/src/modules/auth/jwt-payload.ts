/** Claims carried by an issued access token. */
export interface JwtPayload {
  /** Subject — the OAuth client id. */
  sub: string;
  /** Space-delimited granted scopes. */
  scope: string;
  iss?: string;
  aud?: string;
  exp?: number;
  iat?: number;
}
