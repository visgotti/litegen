// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "meshy/animate",
    displayName: "Meshy Animation",
    mediaType: MediaType.MODEL_3D,
    prices: [{ unit: PriceUnit.PER_REQUEST, amountUsd: 0.06 }],
  },
  {
    id: "meshy/rig",
    displayName: "Meshy Auto Rig",
    mediaType: MediaType.MODEL_3D,
    prices: [{ unit: PriceUnit.PER_REQUEST, amountUsd: 0.1 }],
  },
];
