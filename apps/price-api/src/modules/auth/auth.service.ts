import {
  BadRequestException,
  Injectable,
  Logger,
  UnauthorizedException,
} from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { JwtService } from '@nestjs/jwt';
import { InjectRepository } from '@nestjs/typeorm';
import * as argon2 from 'argon2';
import { randomBytes, randomUUID } from 'crypto';
import { Repository } from 'typeorm';
import { Scope } from '../../common/enums';
import { AppConfig } from '../../config/configuration';
import { OAuthClientEntity } from '../../entities';
import { TokenRequestDto } from './dto/token-request.dto';
import { TokenResponseDto } from './dto/token-response.dto';

/** Result of creating a client — the secret is returned in plaintext once. */
export interface CreatedClient {
  clientId: string;
  clientSecret: string;
  scopes: Scope[];
}

@Injectable()
export class AuthService {
  private readonly logger = new Logger(AuthService.name);

  constructor(
    @InjectRepository(OAuthClientEntity)
    private readonly clientRepo: Repository<OAuthClientEntity>,
    private readonly jwtService: JwtService,
    private readonly configService: ConfigService<AppConfig, true>,
  ) {}

  /** Exchange client credentials for an access token. */
  async issueToken(dto: TokenRequestDto): Promise<TokenResponseDto> {
    const client = await this.clientRepo.findOne({ where: { clientId: dto.client_id } });
    // Verify even when the client is missing/inactive to avoid leaking which
    // case occurred via timing; always return the same generic error.
    const hash = client?.clientSecretHash ?? '$argon2id$v=19$m=65536,t=3,p=4$0000000000000000$0000000000000000000000000000000000000000000';
    const secretOk = await argon2.verify(hash, dto.client_secret).catch(() => false);
    if (!client || !client.active || !secretOk) {
      throw new UnauthorizedException('invalid_client');
    }

    const granted = AuthService.resolveScopes(client.scopes, dto.scope);
    const ttlSeconds = this.configService.get('jwt', { infer: true }).ttlSeconds;
    const access_token = await this.jwtService.signAsync({ scope: granted.join(' ') }, { subject: client.clientId });

    client.lastUsedAt = new Date();
    await this.clientRepo.save(client);

    return {
      access_token,
      token_type: 'Bearer',
      expires_in: ttlSeconds,
      scope: granted.join(' '),
    };
  }

  /**
   * Intersect requested scopes with what the client is permitted. With no
   * `scope` parameter, all client scopes are granted. Requesting a scope the
   * client lacks is rejected. Pure and unit-testable.
   */
  static resolveScopes(clientScopes: Scope[], requested?: string): Scope[] {
    if (!requested || requested.trim() === '') {
      return clientScopes;
    }
    const wanted = requested.split(/\s+/).filter(Boolean);
    const granted = wanted.filter((s) => clientScopes.includes(s as Scope)) as Scope[];
    if (granted.length === 0) {
      throw new BadRequestException('invalid_scope');
    }
    return granted;
  }

  /** Create a new client with a generated id + secret (admin flow). */
  async createClient(name: string, scopes: Scope[]): Promise<CreatedClient> {
    const clientId = `client_${randomUUID()}`;
    const clientSecret = randomBytes(32).toString('base64url');
    await this.clientRepo.save(
      this.clientRepo.create({
        clientId,
        clientSecretHash: await argon2.hash(clientSecret),
        name,
        scopes,
        active: true,
      }),
    );
    this.logger.log(`Created OAuth client ${clientId} with scopes [${scopes.join(', ')}]`);
    return { clientId, clientSecret, scopes };
  }

  /**
   * Idempotently ensure a client with a fixed id + secret exists (bootstrap
   * flow). Used on first boot to seed an initial admin client from env.
   */
  async ensureClient(
    clientId: string,
    clientSecret: string,
    name: string,
    scopes: Scope[],
  ): Promise<void> {
    const existing = await this.clientRepo.findOne({ where: { clientId } });
    if (existing) {
      return;
    }
    await this.clientRepo.save(
      this.clientRepo.create({
        clientId,
        clientSecretHash: await argon2.hash(clientSecret),
        name,
        scopes,
        active: true,
      }),
    );
    this.logger.warn(
      `Bootstrapped OAuth client "${clientId}". Rotate its secret and disable bootstrap in production.`,
    );
  }
}
