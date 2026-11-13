import { ApiProperty } from '@nestjs/swagger';
import { MediaType, ProviderMode } from '../enums';
import { ModelEntity, ProviderEntity } from '../../entities';
import { PriceComponentDto } from './price-component.dto';

/** A model with its full set of price components. */
export class ModelDto {
  @ApiProperty({ example: 'openai/gpt-image-2' })
  id!: string;

  @ApiProperty({ example: 'openai' })
  providerId!: string;

  @ApiProperty({ example: 'GPT Image 2' })
  displayName!: string;

  @ApiProperty({ enum: MediaType, example: MediaType.IMAGE })
  mediaType!: MediaType;

  @ApiProperty({
    enum: ProviderMode,
    description: 'Effective refresh mode (model override, else provider default).',
  })
  mode!: ProviderMode;

  @ApiProperty({
    description:
      'False once litegen no longer carries the model: it is left out of every list, and kept for its price history.',
  })
  active!: boolean;

  @ApiProperty({ type: [PriceComponentDto] })
  prices!: PriceComponentDto[];

  static fromEntity(model: ModelEntity, provider: ProviderEntity): ModelDto {
    const dto = new ModelDto();
    dto.id = model.id;
    dto.providerId = model.providerId;
    dto.displayName = model.displayName;
    dto.mediaType = model.mediaType;
    dto.mode = model.modeOverride ?? provider.mode;
    dto.active = model.active;
    dto.prices = (model.prices ?? []).map((p) => PriceComponentDto.fromEntity(p));
    return dto;
  }
}
