import { ApiProperty } from '@nestjs/swagger';
import { ProviderMode, ScrapeStatus } from '../../../common/enums';
import { ModelDto } from '../../../common/dto/model.dto';
import { ProviderEntity } from '../../../entities';

/** Summary view of a provider. */
export class ProviderDto {
  @ApiProperty({ example: 'openai' })
  id!: string;

  @ApiProperty({ example: 'OpenAI' })
  displayName!: string;

  @ApiProperty({ enum: ProviderMode, description: 'Default refresh mode for the provider.' })
  mode!: ProviderMode;

  @ApiProperty({ nullable: true, example: '0 6 * * *' })
  cronSchedule!: string | null;

  @ApiProperty({ nullable: true, example: 'https://openai.com/api/pricing/' })
  pricingUrl!: string | null;

  @ApiProperty({ description: 'True when a real scraper is wired up (vs. a stub).' })
  scraperImplemented!: boolean;

  @ApiProperty({ nullable: true, format: 'date-time', description: 'Last successful scrape.' })
  lastScrapedAt!: string | null;

  @ApiProperty({ enum: ScrapeStatus, nullable: true })
  lastScrapeStatus!: ScrapeStatus | null;

  @ApiProperty({ example: 3, description: 'Number of models offered by this provider.' })
  modelCount!: number;

  static fromEntity(provider: ProviderEntity, modelCount: number): ProviderDto {
    const dto = new ProviderDto();
    dto.id = provider.id;
    dto.displayName = provider.displayName;
    dto.mode = provider.mode;
    dto.cronSchedule = provider.cronSchedule;
    dto.pricingUrl = provider.pricingUrl;
    dto.scraperImplemented = provider.scraperImplemented;
    dto.lastScrapedAt = provider.lastScrapedAt ? provider.lastScrapedAt.toISOString() : null;
    dto.lastScrapeStatus = provider.lastScrapeStatus;
    dto.modelCount = modelCount;
    return dto;
  }
}

/** Provider with its full model + pricing detail. */
export class ProviderDetailDto extends ProviderDto {
  @ApiProperty({ type: [ModelDto] })
  models!: ModelDto[];

  static fromEntityWithModels(provider: ProviderEntity): ProviderDetailDto {
    const dto = Object.assign(
      new ProviderDetailDto(),
      ProviderDto.fromEntity(provider, provider.models?.length ?? 0),
    );
    dto.models = (provider.models ?? []).map((m) => ModelDto.fromEntity(m, provider));
    return dto;
  }
}
