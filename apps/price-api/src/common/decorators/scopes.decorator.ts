import { SetMetadata } from '@nestjs/common';
import { Scope } from '../enums';

export const SCOPES_KEY = 'requiredScopes';

/**
 * Declares the OAuth scopes a route requires. Enforced by {@link ScopesGuard}.
 * A token must carry every listed scope to be authorised.
 */
export const Scopes = (...scopes: Scope[]) => SetMetadata(SCOPES_KEY, scopes);
