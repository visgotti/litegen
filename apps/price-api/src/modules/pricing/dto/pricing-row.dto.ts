import { ApiProperty } from '@nestjs/swagger';
import { Freshness, MediaType, PriceSource, PriceUnit } from '../../../common/enums';
import { ModelPriceEntity } from '../../../entities';

/** One flattened price row — the primary shape consumers integrate against. */
export class PricingRowDto {
  @ApiProperty({ example: 'openai/dall-e-3' })
  modelId!: string;

  @ApiProperty({ example: 'openai' })
  providerId!: string;

  @ApiProperty({ example: 'DALL-E 3' })
  displayName!: string;

  @ApiProperty({ enum: MediaType })
  mediaType!: MediaType;

  @ApiProperty({ enum: PriceUnit })
  unit!: PriceUnit;

  @ApiProperty({ example: 1 })
  unitAmount!: number;

  @ApiProperty({ example: 0.04 })
  amountUsd!: number;

  @ApiProperty({ example: 'USD' })
  currency!: string;

  @ApiProperty({ nullable: true, type: 'object', additionalProperties: { type: 'string' } })
  tier!: Record<string, string> | null;

  @ApiProperty({ enum: PriceSource })
  source!: PriceSource;

  @ApiProperty({ enum: Freshness })
  freshness!: Freshness;

  @ApiProperty({ format: 'date-time' })
  lastUpdatedAt!: string;

  @ApiProperty({ format: 'date-time', nullable: true })
  lastCheckedAt!: string | null;

  /** Maps a price joined with its model (`price.model` must be loaded). */
  static fromEntity(price: ModelPriceEntity): PricingRowDto {
    const dto = new PricingRowDto();
    dto.modelId = price.modelId;
    dto.providerId = price.model.providerId;
    dto.displayName = price.model.displayName;
    dto.mediaType = price.model.mediaType;
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
