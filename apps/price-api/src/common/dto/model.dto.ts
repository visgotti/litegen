import { ApiProperty } from '@nestjs/swagger';
import { MediaType, ProviderMode } from '../enums';
import { ModelEntity, ProviderEntity } from '../../entities';
import { PriceComponentDto } from './price-component.dto';

/** A model with its full set of price components. */
export class ModelDto {
  @ApiProperty({ example: 'openai/dall-e-3' })
  id!: string;

  @ApiProperty({ example: 'openai' })
  providerId!: string;

  @ApiProperty({ example: 'DALL-E 3' })
  displayName!: string;

  @ApiProperty({ enum: MediaType, example: MediaType.IMAGE })
  mediaType!: MediaType;

  @ApiProperty({
    enum: ProviderMode,
    description: 'Effective refresh mode (model override, else provider default).',
  })
  mode!: ProviderMode;

  @ApiProperty({ type: [PriceComponentDto] })
  prices!: PriceComponentDto[];

  static fromEntity(model: ModelEntity, provider: ProviderEntity): ModelDto {
    const dto = new ModelDto();
    dto.id = model.id;
    dto.providerId = model.providerId;
    dto.displayName = model.displayName;
    dto.mediaType = model.mediaType;
    dto.mode = model.modeOverride ?? provider.mode;
    dto.prices = (model.prices ?? []).map((p) => PriceComponentDto.fromEntity(p));
    return dto;
  }
}
