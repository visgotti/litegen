import { ApiPropertyOptional } from '@nestjs/swagger';
import { IsEnum, IsOptional, IsString } from 'class-validator';
import { Freshness, MediaType } from '../../../common/enums';

/** Query filters for `GET /v1/models`. */
export class ListModelsQuery {
  @ApiPropertyOptional({ example: 'openai', description: 'Filter by provider id.' })
  @IsOptional()
  @IsString()
  provider?: string;

  @ApiPropertyOptional({ enum: MediaType, description: 'Filter by media type.' })
  @IsOptional()
  @IsEnum(MediaType)
  mediaType?: MediaType;

  @ApiPropertyOptional({
    enum: Freshness,
    description: 'Keep only models with at least one price component of this freshness.',
  })
  @IsOptional()
  @IsEnum(Freshness)
  freshness?: Freshness;
}
