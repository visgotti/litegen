import { Injectable, Logger, NotFoundException } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { InjectRepository } from '@nestjs/typeorm';
import { In, Repository } from 'typeorm';
import { AppConfig } from '../../config/configuration';
import { Freshness, PriceSource, ProviderMode, ScrapeStatus } from '../../common/enums';
import { tierKey } from '../../common/tier-key';
import {
  ModelEntity,
  ModelPriceEntity,
  PriceHistoryEntity,
  ProviderEntity,
  ScrapeRunEntity,
} from '../../entities';
import { PROVIDER_REGISTRY, getProviderDefinition } from '../../providers/registry';
import { ProviderDefinition, ScrapedPriceComponent } from '../../providers/types';
import { onFailure, onSuccess } from './freshness';

/**
 * Orchestrates a single provider's scrape and persists the outcome under the
 * freshness rules: successful components are written fresh; failures keep the
 * last-known-good value and degrade it stale → failed; manual-mode models are
 * never overwritten. Every run is recorded as a {@link ScrapeRunEntity}.
 *
 * `registry` and `fetchImpl` are overridable so the resilience behaviour can be
 * unit-tested without a network or a specific provider implementation.
 */
@Injectable()
export class ScrapeService {
  private readonly logger = new Logger(ScrapeService.name);

  /** Overridable for tests; defaults to the real provider registry. */
  registry: ProviderDefinition[] = PROVIDER_REGISTRY;
  /** Overridable for tests; defaults to the platform fetch. */
  fetchImpl: typeof fetch = globalThis.fetch;

  constructor(
    private readonly configService: ConfigService<AppConfig, true>,
    @InjectRepository(ProviderEntity)
    private readonly providerRepo: Repository<ProviderEntity>,
    @InjectRepository(ModelEntity)
    private readonly modelRepo: Repository<ModelEntity>,
    @InjectRepository(ModelPriceEntity)
    private readonly priceRepo: Repository<ModelPriceEntity>,
    @InjectRepository(PriceHistoryEntity)
    private readonly historyRepo: Repository<PriceHistoryEntity>,
    @InjectRepository(ScrapeRunEntity)
    private readonly runRepo: Repository<ScrapeRunEntity>,
  ) {}

  private effectiveMode(model: ModelEntity, provider: ProviderEntity): ProviderMode {
    return model.modeOverride ?? provider.mode;
  }

  /** Scrape every provider that has at least one scraped-mode model. */
  async scrapeAll(): Promise<ScrapeRunEntity[]> {
    const runs: ScrapeRunEntity[] = [];
    for (const def of this.registry) {
      if (def.config.mode === ProviderMode.SCRAPED) {
        runs.push(await this.scrapeProvider(def.config.id));
      }
    }
    return runs;
  }

  /** Scrape a single provider and persist the result. */
  async scrapeProvider(providerId: string): Promise<ScrapeRunEntity> {
    const def = this.registry.find((d) => d.config.id === providerId) ?? getProviderDefinition(providerId);
    if (!def) {
      throw new NotFoundException(`Unknown provider "${providerId}"`);
    }
    const provider = await this.providerRepo.findOne({ where: { id: providerId } });
    if (!provider) {
      throw new NotFoundException(`Provider "${providerId}" is not seeded`);
    }

    const startedAt = new Date();
    const run = this.runRepo.create({
      providerId,
      status: ScrapeStatus.SUCCESS,
      startedAt,
      sourceUrl: def.config.pricingUrl ?? null,
      componentsSeen: 0,
      componentsUpdated: 0,
    });

    // Stub scraper: nothing to refresh, keep curated prices untouched.
    if (!def.scraper.implemented) {
      run.status = ScrapeStatus.SKIPPED;
      provider.lastScrapeStatus = ScrapeStatus.SKIPPED;
      await this.providerRepo.save(provider);
      return this.finish(run, startedAt);
    }

    const models = await this.modelRepo.find({ where: { providerId } });
    const scrapedModels = models.filter((m) => this.effectiveMode(m, provider) === ProviderMode.SCRAPED);

    let components: ScrapedPriceComponent[];
    try {
      components = await def.scraper.scrape({
        timeoutMs: this.configService.get('scraping', { infer: true }).timeoutMs,
        fetch: this.fetchImpl,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.logger.warn(`Scrape failed for ${providerId}: ${message}`);
      await this.markModelsStale(scrapedModels);
      provider.consecutiveFailures += 1;
      provider.lastScrapeStatus = ScrapeStatus.FAILED;
      await this.providerRepo.save(provider);
      run.status = ScrapeStatus.FAILED;
      run.error = message;
      return this.finish(run, startedAt);
    }

    run.componentsSeen = components.length;
    const returned = new Set(components.map((c) => c.modelId));
    let updated = 0;

    for (const component of components) {
      const model = models.find((m) => m.id === component.modelId);
      if (!model) {
        continue; // page listed something we don't track
      }
      if (this.effectiveMode(model, provider) === ProviderMode.MANUAL) {
        continue; // never overwrite a manually-curated model
      }
      const changed = await this.applyComponent(component, run.id);
      if (changed) {
        updated += 1;
      }
    }
    run.componentsUpdated = updated;

    // Scraped models the parser couldn't find this run → degrade those only.
    const missing = scrapedModels.filter((m) => !returned.has(m.id));
    if (missing.length > 0) {
      await this.markModelsStale(missing);
      run.status = ScrapeStatus.PARTIAL;
    }

    provider.lastScrapedAt = new Date();
    provider.lastScrapeStatus = run.status;
    provider.consecutiveFailures = 0;
    await this.providerRepo.save(provider);
    return this.finish(run, startedAt);
  }

  /** Upsert one scraped component; returns true if the amount changed. */
  private async applyComponent(c: ScrapedPriceComponent, runId: string): Promise<boolean> {
    const tier = c.tier ?? null;
    const tk = tierKey(tier);
    const now = new Date();
    const success = onSuccess();

    const existing = await this.priceRepo.findOne({
      where: { modelId: c.modelId, unit: c.unit, tierKey: tk },
    });

    if (existing) {
      const changed = existing.amountUsd !== c.amountUsd;
      existing.source = PriceSource.SCRAPED;
      existing.freshness = success.freshness;
      existing.consecutiveFailures = success.consecutiveFailures;
      existing.lastAttemptAt = now;
      if (changed) {
        const previous = existing.amountUsd;
        existing.amountUsd = c.amountUsd;
        existing.unitAmount = c.unitAmount ?? existing.unitAmount;
        existing.currency = c.currency ?? existing.currency;
        existing.lastUpdatedAt = now;
        await this.priceRepo.save(existing);
        await this.recordHistory(existing, runId, `${previous} -> ${c.amountUsd}`);
        return true;
      }
      await this.priceRepo.save(existing);
      return false;
    }

    const created = this.priceRepo.create({
      modelId: c.modelId,
      unit: c.unit,
      unitAmount: c.unitAmount ?? 1,
      amountUsd: c.amountUsd,
      currency: c.currency ?? 'USD',
      tier,
      tierKey: tk,
      source: PriceSource.SCRAPED,
      freshness: success.freshness,
      consecutiveFailures: 0,
      lastUpdatedAt: now,
      lastAttemptAt: now,
    });
    await this.priceRepo.save(created);
    await this.recordHistory(created, runId, `discovered @ ${c.amountUsd}`);
    return true;
  }

  /** Degrade every price component of the given models toward stale/failed. */
  private async markModelsStale(models: ModelEntity[]): Promise<void> {
    if (models.length === 0) {
      return;
    }
    const staleAfter = this.configService.get('scraping', { infer: true }).staleAfterFailures;
    const now = new Date();
    const prices = await this.priceRepo.find({
      where: { modelId: In(models.map((m) => m.id)) },
    });
    for (const price of prices) {
      const next = onFailure(
        { freshness: price.freshness, consecutiveFailures: price.consecutiveFailures },
        staleAfter,
      );
      price.freshness = next.freshness;
      price.consecutiveFailures = next.consecutiveFailures;
      price.lastAttemptAt = now;
      await this.priceRepo.save(price);
    }
  }

  private async recordHistory(price: ModelPriceEntity, runId: string, note: string): Promise<void> {
    await this.historyRepo.save(
      this.historyRepo.create({
        modelId: price.modelId,
        unit: price.unit,
        unitAmount: price.unitAmount,
        amountUsd: price.amountUsd,
        currency: price.currency,
        tier: price.tier,
        tierKey: price.tierKey,
        source: price.source,
        scrapeRunId: runId,
        note,
        recordedAt: new Date(),
      }),
    );
  }

  private async finish(run: ScrapeRunEntity, startedAt: Date): Promise<ScrapeRunEntity> {
    const finishedAt = new Date();
    run.finishedAt = finishedAt;
    run.durationMs = finishedAt.getTime() - startedAt.getTime();
    return this.runRepo.save(run);
  }
}
