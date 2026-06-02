import { Injectable, UnauthorizedException } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { PassportStrategy } from '@nestjs/passport';
import { ExtractJwt, Strategy } from 'passport-jwt';
import { AuthPrincipal } from '../../common/auth-principal';
import { Scope } from '../../common/enums';
import { AppConfig } from '../../config/configuration';
import { JwtPayload } from './jwt-payload';

/**
 * Validates the bearer access token (HS256) and maps its claims to the
 * {@link AuthPrincipal} attached to `request.user`. Signature, issuer, audience,
 * and expiry are enforced by passport-jwt before `validate` runs.
 */
@Injectable()
export class JwtStrategy extends PassportStrategy(Strategy, 'jwt') {
  constructor(configService: ConfigService<AppConfig, true>) {
    const jwt = configService.get('jwt', { infer: true });
    super({
      jwtFromRequest: ExtractJwt.fromAuthHeaderAsBearerToken(),
      ignoreExpiration: false,
      secretOrKey: jwt.secret,
      issuer: jwt.issuer,
      audience: jwt.audience,
      algorithms: ['HS256'],
    });
  }

  validate(payload: JwtPayload): AuthPrincipal {
    if (!payload?.sub) {
      throw new UnauthorizedException('Invalid token');
    }
    const scopes = (payload.scope ?? '')
      .split(/\s+/)
      .filter(Boolean) as Scope[];
    return { clientId: payload.sub, scopes };
  }
}
