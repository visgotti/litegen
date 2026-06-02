import { Scope } from './enums';

/**
 * The authenticated caller, attached to `request.user` by the JWT strategy.
 * Represents an OAuth client (machine-to-machine), not a human user.
 */
export interface AuthPrincipal {
  clientId: string;
  scopes: Scope[];
}
