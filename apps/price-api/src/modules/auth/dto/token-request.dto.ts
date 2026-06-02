import { ApiProperty, ApiPropertyOptional } from '@nestjs/swagger';
import { IsIn, IsOptional, IsString, MaxLength } from 'class-validator';

/** OAuth2 client-credentials token request body. */
export class TokenRequestDto {
  @ApiProperty({ example: 'client_credentials', enum: ['client_credentials'] })
  @IsIn(['client_credentials'])
  grant_type!: 'client_credentials';

  @ApiProperty({ example: 'bootstrap-admin' })
  @IsString()
  @MaxLength(128)
  client_id!: string;

  @ApiProperty({ example: 's3cr3t', writeOnly: true })
  @IsString()
  @MaxLength(256)
  client_secret!: string;

  @ApiPropertyOptional({
    example: 'pricing:read',
    description: 'Space-delimited subset of the client scopes to request. Defaults to all.',
  })
  @IsOptional()
  @IsString()
  scope?: string;
}
