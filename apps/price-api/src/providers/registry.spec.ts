import { PROVIDER_REGISTRY, allModelIds } from './registry';

/**
 * Registry-level invariants. This is a fast, network-free guard that complements
 * the yaml-driven `coverage:check` script: it ensures the registry itself is
 * internally consistent (unique, well-formed ids; every model prefixed by its
 * provider; at least one price per model).
 */
describe('provider registry', () => {
  it('declares the expected providers', () => {
    const ids = PROVIDER_REGISTRY.map((p) => p.config.id).sort();
    expect(ids).toEqual(['fal', 'google', 'luma', 'openai', 'replicate', 'runway', 'stability']);
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

  it('covers all 32 currently-supported models', () => {
    expect(allModelIds().size).toBe(32);
  });
});
