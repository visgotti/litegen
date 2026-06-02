import {
  CanActivate,
  ExecutionContext,
  ForbiddenException,
  Injectable,
} from '@nestjs/common';
import { Reflector } from '@nestjs/core';
import { AuthPrincipal } from '../auth-principal';
import { IS_PUBLIC_KEY } from '../decorators/public.decorator';
import { SCOPES_KEY } from '../decorators/scopes.decorator';
import { Scope } from '../enums';

/**
 * Authorisation guard. Enforces that the authenticated principal carries every
 * scope declared via `@Scopes(...)`. Runs after {@link JwtAuthGuard}, so
 * `request.user` is already populated for protected routes.
 */
@Injectable()
export class ScopesGuard implements CanActivate {
  constructor(private readonly reflector: Reflector) {}

  canActivate(context: ExecutionContext): boolean {
    const isPublic = this.reflector.getAllAndOverride<boolean>(IS_PUBLIC_KEY, [
      context.getHandler(),
      context.getClass(),
    ]);
    if (isPublic) {
      return true;
    }

    const required = this.reflector.getAllAndOverride<Scope[]>(SCOPES_KEY, [
      context.getHandler(),
      context.getClass(),
    ]);
    if (!required || required.length === 0) {
      return true;
    }

    const request = context.switchToHttp().getRequest();
    const principal: AuthPrincipal | undefined = request.user;
    const held = new Set(principal?.scopes ?? []);
    const missing = required.filter((scope) => !held.has(scope));

    if (missing.length > 0) {
      throw new ForbiddenException(`Missing required scope(s): ${missing.join(', ')}`);
    }
    return true;
  }
}
