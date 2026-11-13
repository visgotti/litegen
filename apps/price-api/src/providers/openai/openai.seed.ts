// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "openai/gpt-image-2",
    displayName: "GPT Image 2",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.053 }],
  },
  {
    id: "openai/gpt-image-2.5-flare",
    displayName: "GPT Image 2.5 Flare",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.0132 }],
  },
  {
    id: "openai/gpt-image-2.5-sunburst",
    displayName: "GPT Image 2.5 Sunburst",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.0132 }],
  },
];
