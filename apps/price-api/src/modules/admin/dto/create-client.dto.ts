import { ApiProperty } from '@nestjs/swagger';
import { ArrayNotEmpty, IsArray, IsEnum, IsString, MaxLength } from 'class-validator';
import { Scope } from '../../../common/enums';

/** Request to create a new OAuth client. */
export class CreateClientDto {
  @ApiProperty({ example: 'billing-service' })
  @IsString()
  @MaxLength(128)
  name!: string;

  @ApiProperty({ enum: Scope, isArray: true, example: [Scope.READ] })
  @IsArray()
  @ArrayNotEmpty()
  @IsEnum(Scope, { each: true })
  scopes!: Scope[];
}

/** Response — the secret is shown only once. */
export class CreatedClientDto {
  @ApiProperty()
  clientId!: string;

  @ApiProperty({ description: 'Plaintext secret — store it now; it cannot be retrieved later.' })
  clientSecret!: string;

  @ApiProperty({ enum: Scope, isArray: true })
  scopes!: Scope[];
}
