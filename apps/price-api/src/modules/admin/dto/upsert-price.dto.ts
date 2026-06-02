import { ApiProperty, ApiPropertyOptional } from '@nestjs/swagger';
import {
  IsEnum,
  IsNumber,
  IsObject,
  IsOptional,
  IsString,
  Min,
} from 'class-validator';
import { PriceUnit } from '../../../common/enums';

/** Manually set/replace one price component for a model. */
export class UpsertPriceDto {
  @ApiProperty({ enum: PriceUnit })
  @IsEnum(PriceUnit)
  unit!: PriceUnit;

  @ApiProperty({ example: 0.045 })
  @IsNumber()
  @Min(0)
  amountUsd!: number;

  @ApiPropertyOptional({ example: 1, default: 1 })
  @IsOptional()
  @IsNumber()
  @Min(0)
  unitAmount?: number;

  @ApiPropertyOptional({ example: 'USD', default: 'USD' })
  @IsOptional()
  @IsString()
  currency?: string;

  @ApiPropertyOptional({
    type: 'object',
    additionalProperties: { type: 'string' },
    example: { resolution: '1080p' },
    description: 'Optional tier qualifiers; identifies the component alongside unit.',
  })
  @IsOptional()
  @IsObject()
  tier?: Record<string, string>;
}
