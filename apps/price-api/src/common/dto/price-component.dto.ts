import { ApiProperty } from '@nestjs/swagger';
import { Freshness, PriceSource, PriceUnit } from '../enums';
import { ModelPriceEntity } from '../../entities';

/** One billing component of a model's price, as returned by the API. */
export class PriceComponentDto {
  @ApiProperty({ enum: PriceUnit, example: PriceUnit.PER_IMAGE })
  unit!: PriceUnit;

  @ApiProperty({ example: 1, description: 'Denominator the price applies to (per N units).' })
  unitAmount!: number;

  @ApiProperty({ example: 0.04, description: 'Price in USD per `unitAmount` of `unit`.' })
  amountUsd!: number;

  @ApiProperty({ example: 'USD' })
  currency!: string;

  @ApiProperty({
    nullable: true,
    type: 'object',
    additionalProperties: { type: 'string' },
    example: { resolution: '1080p' },
    description: 'Optional tier qualifiers; null when the price is untiered.',
  })
  tier!: Record<string, string> | null;

  @ApiProperty({ enum: PriceSource, example: PriceSource.SCRAPED })
  source!: PriceSource;

  @ApiProperty({ enum: Freshness, example: Freshness.FRESH })
  freshness!: Freshness;

  @ApiProperty({ format: 'date-time', description: 'When the amount last changed.' })
  lastUpdatedAt!: string;

  @ApiProperty({
    format: 'date-time',
    nullable: true,
    description: 'When a refresh was last attempted (success or failure).',
  })
  lastCheckedAt!: string | null;

  static fromEntity(price: ModelPriceEntity): PriceComponentDto {
    const dto = new PriceComponentDto();
    dto.unit = price.unit;
    dto.unitAmount = price.unitAmount;
    dto.amountUsd = price.amountUsd;
    dto.currency = price.currency;
    dto.tier = price.tier ?? null;
    dto.source = price.source;
    dto.freshness = price.freshness;
    dto.lastUpdatedAt = price.lastUpdatedAt.toISOString();
    dto.lastCheckedAt = price.lastAttemptAt ? price.lastAttemptAt.toISOString() : null;
    return dto;
  }
}
