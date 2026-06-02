import { Injectable, NotFoundException } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';
import { PriceComponentDto } from '../../common/dto/price-component.dto';
import { Freshness, PriceSource } from '../../common/enums';
import { tierKey } from '../../common/tier-key';
import {
  ModelEntity,
  ModelPriceEntity,
  PriceHistoryEntity,
  ProviderEntity,
} from '../../entities';
import { ProviderDto } from '../providers/dto/provider.dto';
import { ScrapeService } from '../scraping/scrape.service';
import { ScrapeRunDto } from './dto/scrape-run.dto';
import { UpdateProviderDto } from './dto/update-provider.dto';
import { UpsertPriceDto } from './dto/upsert-price.dto';

@Injectable()
export class AdminService {
  constructor(
    @InjectRepository(ProviderEntity)
    private readonly providerRepo: Repository<ProviderEntity>,
    @InjectRepository(ModelEntity)
    private readonly modelRepo: Repository<ModelEntity>,
    @InjectRepository(ModelPriceEntity)
    private readonly priceRepo: Repository<ModelPriceEntity>,
    @InjectRepository(PriceHistoryEntity)
    private readonly historyRepo: Repository<PriceHistoryEntity>,
    private readonly scrapeService: ScrapeService,
  ) {}

  /** Manually set a model's price component. Marks it source=manual, fresh. */
  async upsertPrice(
    modelId: string,
    dto: UpsertPriceDto,
    actor: string,
  ): Promise<PriceComponentDto> {
    const model = await this.modelRepo.findOne({ where: { id: modelId } });
    if (!model) {
      throw new NotFoundException(`Model "${modelId}" not found`);
    }
    const tier = dto.tier ?? null;
    const tk = tierKey(tier);
    const now = new Date();

    let price = await this.priceRepo.findOne({
      where: { modelId, unit: dto.unit, tierKey: tk },
    });
    const previous = price?.amountUsd ?? null;

    if (!price) {
      price = this.priceRepo.create({ modelId, unit: dto.unit, tier, tierKey: tk });
    }
    price.amountUsd = dto.amountUsd;
    price.unitAmount = dto.unitAmount ?? price.unitAmount ?? 1;
    price.currency = dto.currency ?? price.currency ?? 'USD';
    price.tier = tier;
    price.source = PriceSource.MANUAL;
    price.freshness = Freshness.FRESH;
    price.consecutiveFailures = 0;
    price.lastUpdatedAt = now;
    price.lastAttemptAt = now;
    const saved = await this.priceRepo.save(price);

    await this.historyRepo.save(
      this.historyRepo.create({
        modelId,
        unit: saved.unit,
        unitAmount: saved.unitAmount,
        amountUsd: saved.amountUsd,
        currency: saved.currency,
        tier: saved.tier,
        tierKey: saved.tierKey,
        source: PriceSource.MANUAL,
        scrapeRunId: null,
        note: `manual upsert by ${actor}${previous !== null ? ` (${previous} -> ${saved.amountUsd})` : ''}`,
        recordedAt: now,
      }),
    );
    return PriceComponentDto.fromEntity(saved);
  }

  /** Patch a provider's refresh config. */
  async updateProvider(id: string, dto: UpdateProviderDto): Promise<ProviderDto> {
    const provider = await this.providerRepo.findOne({ where: { id } });
    if (!provider) {
      throw new NotFoundException(`Provider "${id}" not found`);
    }
    if (dto.mode !== undefined) {
      provider.mode = dto.mode;
    }
    if (dto.cronSchedule !== undefined) {
      provider.cronSchedule = dto.cronSchedule.trim() === '' ? null : dto.cronSchedule;
    }
    if (dto.pricingUrl !== undefined) {
      provider.pricingUrl = dto.pricingUrl;
    }
    if (dto.notes !== undefined) {
      provider.notes = dto.notes;
    }
    const saved = await this.providerRepo.save(provider);
    const modelCount = await this.modelRepo.count({ where: { providerId: id } });
    return ProviderDto.fromEntity(saved, modelCount);
  }

  /** Trigger an immediate scrape for a provider. */
  async triggerScrape(id: string): Promise<ScrapeRunDto> {
    const run = await this.scrapeService.scrapeProvider(id);
    return ScrapeRunDto.fromEntity(run);
  }
}
