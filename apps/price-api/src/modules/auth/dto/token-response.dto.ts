import { ApiProperty } from '@nestjs/swagger';

/** OAuth2 token response. */
export class TokenResponseDto {
  @ApiProperty({ description: 'Signed JWT access token (HS256).' })
  access_token!: string;

  @ApiProperty({ example: 'Bearer' })
  token_type!: 'Bearer';

  @ApiProperty({ example: 3600, description: 'Token lifetime in seconds.' })
  expires_in!: number;

  @ApiProperty({ example: 'pricing:read pricing:admin', description: 'Granted scopes.' })
  scope!: string;
}
