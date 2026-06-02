import { BadRequestException } from '@nestjs/common';
import { Scope } from '../../common/enums';
import { AuthService } from './auth.service';

describe('AuthService.resolveScopes', () => {
  it('grants all client scopes when none requested', () => {
    expect(AuthService.resolveScopes([Scope.READ, Scope.ADMIN])).toEqual([Scope.READ, Scope.ADMIN]);
  });

  it('grants all client scopes for an empty scope string', () => {
    expect(AuthService.resolveScopes([Scope.READ], '   ')).toEqual([Scope.READ]);
  });

  it('intersects requested scopes with the client grant', () => {
    expect(AuthService.resolveScopes([Scope.READ, Scope.ADMIN], 'pricing:read')).toEqual([
      Scope.READ,
    ]);
  });

  it('rejects a request for a scope the client does not hold', () => {
    expect(() => AuthService.resolveScopes([Scope.READ], 'pricing:admin')).toThrow(
      BadRequestException,
    );
  });
});
