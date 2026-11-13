import { LitegenModel } from './litegen-models';

/** The enum names a catalog model's media type and price unit map to. */
function seedKinds(mediaType: string): { media: string; unit: string } {
  switch (mediaType) {
    case 'video':
      return { media: 'MediaType.VIDEO', unit: 'PriceUnit.PER_VIDEO' };
    case 'model3d':
      return { media: 'MediaType.MODEL_3D', unit: 'PriceUnit.PER_REQUEST' };
    default:
      return { media: 'MediaType.IMAGE', unit: 'PriceUnit.PER_IMAGE' };
  }
}

/**
 * The source of `src/providers/<provider>/<provider>.seed.ts`: every model
 * litegen's catalog carries for the provider, at its default request's price
 * (`pricing.base_cost_usd`), in the unit its medium bills by. The catalog is
 * the source; the file says so.
 */
export function renderSeed(models: LitegenModel[]): string {
  const entries = [...models]
    .sort((a, b) => a.id.localeCompare(b.id))
    .map((m) => {
      const { media, unit } = seedKinds(m.mediaType);
      return `  {
    id: ${JSON.stringify(m.id)},
    displayName: ${JSON.stringify(m.displayName)},
    mediaType: ${media},
    prices: [{ unit: ${unit}, amountUsd: ${m.baseCostUsd ?? 0} }],
  },`;
    })
    .join('\n');
  return `// GENERATED from litegen's models/*.yaml by \`npm run coverage:fix\`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
${entries}
];
`;
}
