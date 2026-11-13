// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "pixellab/pixflux",
    displayName: "PixelLab Pixflux",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.013 }],
  },
];
