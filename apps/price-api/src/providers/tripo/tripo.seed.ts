// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "tripo/animate",
    displayName: "Tripo Animation Retarget",
    mediaType: MediaType.MODEL_3D,
    prices: [{ unit: PriceUnit.PER_REQUEST, amountUsd: 0.1 }],
  },
  {
    id: "tripo/multiview",
    displayName: "Tripo Image to Multiview",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.1 }],
  },
  {
    id: "tripo/rig",
    displayName: "Tripo Auto Rig",
    mediaType: MediaType.MODEL_3D,
    prices: [{ unit: PriceUnit.PER_REQUEST, amountUsd: 0.25 }],
  },
];
