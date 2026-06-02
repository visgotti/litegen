import { Freshness, PriceSource, PriceUnit, ProviderMode, ScrapeStatus } from '../../common/enums';
import { StubScraper } from '../../providers/base';
import { ProviderDefinition, ProviderScraper } from '../../providers/types';
import { ScrapeService } from './scrape.service';

/**
 * Verifies the resilience guarantees without a database:
 *  - a stub scraper is SKIPPED and never touches prices
 *  - a thrown scrape degrades scraped models to stale (keeps the value)
 *  - a partial scrape updates what it found and degrades only what it missed
 */
describe('ScrapeService resilience', () => {
  function setup() {
    const provider: any = {
      id: 'p1',
      mode: ProviderMode.SCRAPED,
      consecutiveFailures: 0,
      lastScrapeStatus: null,
      lastScrapedAt: null,
    };
    const models: any[] = [
      { id: 'p1/a', providerId: 'p1', modeOverride: null },
      { id: 'p1/b', providerId: 'p1', modeOverride: null },
    ];
    const prices: any[] = [
      {
        modelId: 'p1/a',
        unit: PriceUnit.PER_IMAGE,
        tierKey: '*',
        amountUsd: 1,
        unitAmount: 1,
        currency: 'USD',
        freshness: Freshness.FRESH,
        source: PriceSource.FALLBACK,
        consecutiveFailures: 0,
      },
      {
        modelId: 'p1/b',
        unit: PriceUnit.PER_IMAGE,
        tierKey: '*',
        amountUsd: 5,
        unitAmount: 1,
        currency: 'USD',
        freshness: Freshness.FRESH,
        source: PriceSource.FALLBACK,
        consecutiveFailures: 0,
      },
    ];

    const priceSave = jest.fn(async (p: any) => p);
    const providerSave = jest.fn(async (p: any) => p);

    const configService: any = {
      get: () => ({ timeoutMs: 1000, staleAfterFailures: 3 }),
    };
    const providerRepo: any = { findOne: async () => provider, save: providerSave };
    const modelRepo: any = { find: async () => models };
    const priceRepo: any = {
      find: async (query?: any) => {
        const op = query?.where?.modelId;
        if (op && Array.isArray(op.value)) {
          return prices.filter((p) => op.value.includes(p.modelId));
        }
        return prices;
      },
      findOne: async ({ where }: any) =>
        prices.find(
          (p) => p.modelId === where.modelId && p.unit === where.unit && p.tierKey === where.tierKey,
        ) ?? null,
      create: (x: any) => ({ ...x }),
      save: priceSave,
    };
    const historyRepo: any = { create: (x: any) => x, save: jest.fn(async () => undefined) };
    const runRepo: any = {
      create: (x: any) => ({ ...x }),
      save: jest.fn(async (r: any) => ({ id: 'run1', ...r })),
    };

    const service = new ScrapeService(
      configService,
      providerRepo,
      modelRepo,
      priceRepo,
      historyRepo,
      runRepo,
    );
    return { service, provider, models, prices, priceSave, providerSave };
  }

  function registryWith(scraper: ProviderScraper, pricingUrl: string | null): ProviderDefinition[] {
    return [
      {
        config: { id: 'p1', displayName: 'P1', mode: ProviderMode.SCRAPED, pricingUrl },
        seed: [],
        scraper,
      },
    ];
  }

  it('skips a stub scraper without touching prices', async () => {
    const { service, provider, priceSave } = setup();
    service.registry = registryWith(new StubScraper('p1'), null);

    const run = await service.scrapeProvider('p1');

    expect(run.status).toBe(ScrapeStatus.SKIPPED);
    expect(priceSave).not.toHaveBeenCalled();
    expect(provider.lastScrapeStatus).toBe(ScrapeStatus.SKIPPED);
  });

  it('degrades scraped models to stale when the scrape throws, keeping prices', async () => {
    const { service, provider, prices } = setup();
    const throwing: ProviderScraper = {
      providerId: 'p1',
      implemented: true,
      scrape: async () => {
        throw new Error('boom');
      },
    };
    service.registry = registryWith(throwing, 'http://x');

    const run = await service.scrapeProvider('p1');

    expect(run.status).toBe(ScrapeStatus.FAILED);
    expect(run.error).toContain('boom');
    expect(provider.consecutiveFailures).toBe(1);
    for (const price of prices) {
      expect(price.freshness).toBe(Freshness.STALE);
      expect(price.consecutiveFailures).toBe(1);
      expect(price.amountUsd).toBeGreaterThan(0); // value preserved
    }
  });

  it('updates found components and degrades only the missing ones (partial)', async () => {
    const { service, provider, prices } = setup();
    const partial: ProviderScraper = {
      providerId: 'p1',
      implemented: true,
      // returns only model a, with a changed price; model b is missing
      scrape: async () => [{ modelId: 'p1/a', unit: PriceUnit.PER_IMAGE, amountUsd: 2 }],
    };
    service.registry = registryWith(partial, 'http://x');

    const run = await service.scrapeProvider('p1');

    expect(run.status).toBe(ScrapeStatus.PARTIAL);
    expect(run.componentsSeen).toBe(1);
    expect(run.componentsUpdated).toBe(1);

    const a = prices.find((p) => p.modelId === 'p1/a');
    const b = prices.find((p) => p.modelId === 'p1/b');
    expect(a.amountUsd).toBe(2);
    expect(a.freshness).toBe(Freshness.FRESH);
    expect(a.source).toBe(PriceSource.SCRAPED);
    expect(b.freshness).toBe(Freshness.STALE);
    expect(b.consecutiveFailures).toBe(1);
    expect(provider.lastScrapeStatus).toBe(ScrapeStatus.PARTIAL);
  });
});
