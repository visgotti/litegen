import { MediaType, PriceUnit, ProviderMode } from '../../src/common/enums';
import { StubScraper } from '../../src/providers/base';
import { ProviderDefinition } from '../../src/providers/types';
import { findGaps } from './coverage';
import { LitegenModel } from './litegen-models';

const catalog = (id: string): LitegenModel => ({
  id,
  provider: id.split('/')[0],
  displayName: id,
  mediaType: 'image',
  baseCostUsd: 0.01,
});

const provider = (id: string, models: string[]): ProviderDefinition => ({
  config: { id, displayName: id, mode: ProviderMode.MANUAL },
  seed: models.map((m) => ({
    id: m,
    displayName: m,
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.01 }],
  })),
  scraper: new StubScraper(id),
});

describe('coverage gaps', () => {
  it('names a missing provider, an unseeded model and a seeded model the catalog dropped', () => {
    const gaps = findGaps(
      [catalog('a/x'), catalog('a/y'), catalog('b/z')],
      [provider('a', ['a/x', 'a/old'])],
    );
    expect(gaps.providerMissing).toEqual(['b']);
    expect(gaps.modelsMissing.map((m) => m.id)).toEqual(['a/y', 'b/z']);
    expect(gaps.modelsRetired).toEqual(['a/old']);
  });

  it('finds nothing when the registry mirrors the catalog', () => {
    const gaps = findGaps([catalog('a/x'), catalog('b/z')], [provider('a', ['a/x']), provider('b', ['b/z'])]);
    expect(gaps).toEqual({ providerMissing: [], modelsMissing: [], modelsRetired: [] });
  });
});
