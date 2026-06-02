import { ApiPropertyOptional } from '@nestjs/swagger';
import { IsEnum, IsOptional, IsString } from 'class-validator';
import { Freshness, MediaType, PriceSource, PriceUnit } from '../../../common/enums';

/** Query filters for `GET /v1/pricing`. */
export class PricingQuery {
  @ApiPropertyOptional({ example: 'openai' })
  @IsOptional()
  @IsString()
  provider?: string;

  @ApiPropertyOptional({ enum: MediaType })
  @IsOptional()
  @IsEnum(MediaType)
  mediaType?: MediaType;

  @ApiPropertyOptional({ enum: PriceUnit })
  @IsOptional()
  @IsEnum(PriceUnit)
  unit?: PriceUnit;

  @ApiPropertyOptional({ enum: Freshness })
  @IsOptional()
  @IsEnum(Freshness)
  freshness?: Freshness;

  @ApiPropertyOptional({ enum: PriceSource })
  @IsOptional()
  @IsEnum(PriceSource)
  source?: PriceSource;
}
