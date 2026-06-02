import { Injectable, Logger, OnApplicationBootstrap } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';
import { Freshness, PriceSource, ProviderMode, Scope } from '../../common/enums';
import { tierKey } from '../../common/tier-key';
import { AppConfig } from '../../config/configuration';
import { ModelEntity, ModelPriceEntity, PriceHistoryEntity, ProviderEntity } from '../../entities';
import { PROVIDER_REGISTRY } from '../../providers/registry';
import { AuthService } from '../auth/auth.service';

/**
 * Idempotently materialises the provider registry into the database on boot:
 * providers, models, and any missing baseline price components. Existing prices
 * are never overwritten — scraped/manual updates survive restarts. Operator
 * edits to provider mode/cron/url are also preserved; only static facts
 * (display name, scraper-implemented flag, model metadata) are refreshed.
 *
 * Also bootstraps an initial admin OAuth client when configured.
 */
@Injectable()
export class SeedService implements OnApplicationBootstrap {
  private readonly logger = new Logger(SeedService.name);

  constructor(
    private readonly configService: ConfigService<AppConfig, true>,
    private readonly authService: AuthService,
    @InjectRepository(ProviderEntity)
    private readonly providerRepo: Repository<ProviderEntity>,
    @InjectRepository(ModelEntity)
    private readonly modelRepo: Repository<ModelEntity>,
    @InjectRepository(ModelPriceEntity)
    private readonly priceRepo: Repository<ModelPriceEntity>,
    @InjectRepository(PriceHistoryEntity)
    private readonly historyRepo: Repository<PriceHistoryEntity>,
  ) {}

  async onApplicationBootstrap(): Promise<void> {
    await this.seed();
    await this.bootstrapClient();
  }

  /** Insert/refresh providers, models, and baseline prices from the registry. */
  async seed(): Promise<{ providers: number; models: number; pricesInserted: number }> {
    let pricesInserted = 0;
    let modelCount = 0;

    for (const def of PROVIDER_REGISTRY) {
      await this.upsertProvider(def.config.id, def);

      for (const sm of def.seed) {
        modelCount += 1;
        await this.upsertModel(sm, def.config.id);

        const effectiveMode = sm.modeOverride ?? def.config.mode;
        for (const sp of sm.prices) {
          const tier = sp.tier ?? null;
          const tk = tierKey(tier);
          const exists = await this.priceRepo.findOne({
            where: { modelId: sm.id, unit: sp.unit, tierKey: tk },
          });
          if (exists) {
            continue;
          }
          const now = new Date();
          const saved = await this.priceRepo.save(
            this.priceRepo.create({
              modelId: sm.id,
              unit: sp.unit,
              unitAmount: sp.unitAmount ?? 1,
              amountUsd: sp.amountUsd,
              currency: sp.currency ?? 'USD',
              tier,
              tierKey: tk,
              source: effectiveMode === ProviderMode.MANUAL ? PriceSource.MANUAL : PriceSource.FALLBACK,
              freshness: Freshness.FRESH,
              consecutiveFailures: 0,
              lastUpdatedAt: now,
              lastAttemptAt: null,
            }),
          );
          await this.historyRepo.save(
            this.historyRepo.create({
              modelId: sm.id,
              unit: saved.unit,
              unitAmount: saved.unitAmount,
              amountUsd: saved.amountUsd,
              currency: saved.currency,
              tier: saved.tier,
              tierKey: saved.tierKey,
              source: saved.source,
              scrapeRunId: null,
              note: 'seed baseline',
              recordedAt: now,
            }),
          );
          pricesInserted += 1;
        }
      }
    }

    this.logger.log(
      `Seed complete: ${PROVIDER_REGISTRY.length} providers, ${modelCount} models, ${pricesInserted} baseline prices inserted.`,
    );
    return { providers: PROVIDER_REGISTRY.length, models: modelCount, pricesInserted };
  }

  private async upsertProvider(
    id: string,
    def: (typeof PROVIDER_REGISTRY)[number],
  ): Promise<void> {
    const existing = await this.providerRepo.findOne({ where: { id } });
    if (!existing) {
      await this.providerRepo.save(
        this.providerRepo.create({
          id,
          displayName: def.config.displayName,
          mode: def.config.mode,
          cronSchedule: def.config.cronSchedule ?? null,
          pricingUrl: def.config.pricingUrl ?? null,
          notes: def.config.notes ?? null,
          scraperImplemented: def.scraper.implemented,
        }),
      );
      return;
    }
    // Refresh only static facts; preserve operator-editable config.
    existing.displayName = def.config.displayName;
    existing.scraperImplemented = def.scraper.implemented;
    await this.providerRepo.save(existing);
  }

  private async upsertModel(
    sm: (typeof PROVIDER_REGISTRY)[number]['seed'][number],
    providerId: string,
  ): Promise<void> {
    const existing = await this.modelRepo.findOne({ where: { id: sm.id } });
    if (!existing) {
      await this.modelRepo.save(
        this.modelRepo.create({
          id: sm.id,
          providerId,
          displayName: sm.displayName,
          mediaType: sm.mediaType,
          modeOverride: sm.modeOverride ?? null,
          active: true,
        }),
      );
      return;
    }
    existing.displayName = sm.displayName;
    existing.mediaType = sm.mediaType;
    existing.modeOverride = sm.modeOverride ?? null;
    await this.modelRepo.save(existing);
  }

  private async bootstrapClient(): Promise<void> {
    const { clientId, clientSecret, scopes } = this.configService.get('bootstrap', { infer: true });
    if (!clientSecret) {
      return;
    }
    await this.authService.ensureClient(clientId, clientSecret, 'bootstrap', scopes as Scope[]);
  }
}
