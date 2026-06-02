import { ExecutionContext, ForbiddenException } from '@nestjs/common';
import { Reflector } from '@nestjs/core';
import { AuthPrincipal } from '../auth-principal';
import { Scope } from '../enums';
import { IS_PUBLIC_KEY } from '../decorators/public.decorator';
import { SCOPES_KEY } from '../decorators/scopes.decorator';
import { ScopesGuard } from './scopes.guard';

function makeContext(user?: Partial<AuthPrincipal>): ExecutionContext {
  return {
    switchToHttp: () => ({ getRequest: () => ({ user }) }),
    getHandler: () => undefined,
    getClass: () => undefined,
  } as unknown as ExecutionContext;
}

function makeReflector(values: Record<string, unknown>): Reflector {
  return {
    getAllAndOverride: (key: string) => values[key],
  } as unknown as Reflector;
}

describe('ScopesGuard', () => {
  it('allows public routes regardless of scopes', () => {
    const guard = new ScopesGuard(makeReflector({ [IS_PUBLIC_KEY]: true }));
    expect(guard.canActivate(makeContext())).toBe(true);
  });

  it('allows routes with no required scopes', () => {
    const guard = new ScopesGuard(makeReflector({ [SCOPES_KEY]: [] }));
    expect(guard.canActivate(makeContext({ clientId: 'c', scopes: [] }))).toBe(true);
  });

  it('allows when the principal holds every required scope', () => {
    const guard = new ScopesGuard(makeReflector({ [SCOPES_KEY]: [Scope.ADMIN] }));
    expect(guard.canActivate(makeContext({ clientId: 'c', scopes: [Scope.READ, Scope.ADMIN] }))).toBe(
      true,
    );
  });

  it('forbids when a required scope is missing', () => {
    const guard = new ScopesGuard(makeReflector({ [SCOPES_KEY]: [Scope.ADMIN] }));
    expect(() => guard.canActivate(makeContext({ clientId: 'c', scopes: [Scope.READ] }))).toThrow(
      ForbiddenException,
    );
  });
});
