import { ApiProperty } from '@nestjs/swagger';
import { PriceSource, PriceUnit } from '../../../common/enums';
import { PriceHistoryEntity } from '../../../entities';

/** A single historical price observation for a model component. */
export class PriceHistoryDto {
  @ApiProperty({ enum: PriceUnit })
  unit!: PriceUnit;

  @ApiProperty({ example: 0.04 })
  amountUsd!: number;

  @ApiProperty({ example: 'USD' })
  currency!: string;

  @ApiProperty({ nullable: true, type: 'object', additionalProperties: { type: 'string' } })
  tier!: Record<string, string> | null;

  @ApiProperty({ enum: PriceSource })
  source!: PriceSource;

  @ApiProperty({ nullable: true, example: '0.04 -> 0.045' })
  note!: string | null;

  @ApiProperty({ format: 'date-time' })
  recordedAt!: string;

  static fromEntity(h: PriceHistoryEntity): PriceHistoryDto {
    const dto = new PriceHistoryDto();
    dto.unit = h.unit;
    dto.amountUsd = h.amountUsd;
    dto.currency = h.currency;
    dto.tier = h.tier ?? null;
    dto.source = h.source;
    dto.note = h.note ?? null;
    dto.recordedAt = h.recordedAt.toISOString();
    return dto;
  }
}
