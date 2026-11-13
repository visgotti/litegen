import { ProviderMode } from '../common/enums';
import { PROVIDER_REGISTRY, allModelIds } from './registry';

/**
 * Registry-level invariants. This is a fast, network-free guard that complements
 * the yaml-driven `coverage:check` script (which compares the registry with
 * litegen's catalog): it ensures the registry itself is internally consistent.
 */
describe('provider registry', () => {
  it('registers each provider once', () => {
    const ids = PROVIDER_REGISTRY.map((p) => p.config.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('has unique model ids', () => {
    const ids = [...allModelIds()];
    const all = PROVIDER_REGISTRY.flatMap((p) => p.seed.map((m) => m.id));
    expect(all).toHaveLength(ids.length);
  });

  it('prefixes every model id with its provider id', () => {
    for (const def of PROVIDER_REGISTRY) {
      for (const model of def.seed) {
        expect(model.id.startsWith(`${def.config.id}/`)).toBe(true);
      }
    }
  });

  it('gives every model at least one price component', () => {
    for (const def of PROVIDER_REGISTRY) {
      for (const model of def.seed) {
        expect(model.prices.length).toBeGreaterThan(0);
        for (const price of model.prices) {
          expect(price.amountUsd).toBeGreaterThanOrEqual(0);
        }
      }
    }
  });

  it("wires each provider's scraper to its own id", () => {
    for (const def of PROVIDER_REGISTRY) {
      expect(def.scraper.providerId).toBe(def.config.id);
    }
  });

  it('has each scraper report only models its provider seeds', () => {
    for (const def of PROVIDER_REGISTRY) {
      const seeded = new Set(def.seed.map((m) => m.id));
      const strays = def.scraper.modelIds.filter((id) => !seeded.has(id));
      expect([def.config.id, strays]).toEqual([def.config.id, []]);
    }
  });

  it('pins every model a real scraper does not report to manual', () => {
    // Otherwise each scrape would mark a price it never looks at stale.
    for (const def of PROVIDER_REGISTRY.filter((d) => d.scraper.implemented)) {
      const scraped = new Set(def.scraper.modelIds);
      const unpinned = def.seed
        .filter((m) => !scraped.has(m.id) && m.modeOverride !== ProviderMode.MANUAL)
        .map((m) => m.id);
      expect([def.config.id, unpinned]).toEqual([def.config.id, []]);
    }
  });
});
