// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "recraft/recraftv2",
    displayName: "Recraft V2",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.022 }],
  },
  {
    id: "recraft/recraftv2_vector",
    displayName: "Recraft V2 Vector",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.044 }],
  },
  {
    id: "recraft/recraftv3",
    displayName: "Recraft V3",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: "recraft/recraftv3_vector",
    displayName: "Recraft V3 Vector",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.08 }],
  },
  {
    id: "recraft/recraftv4_1",
    displayName: "Recraft V4.1",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.035 }],
  },
  {
    id: "recraft/recraftv4_1_flash",
    displayName: "Recraft V4.1 Flash",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.007 }],
  },
  {
    id: "recraft/recraftv4_1_pro",
    displayName: "Recraft V4.1 Pro",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.21 }],
  },
  {
    id: "recraft/recraftv4_1_pro_vector",
    displayName: "Recraft V4.1 Pro Vector",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.3 }],
  },
  {
    id: "recraft/recraftv4_1_vector",
    displayName: "Recraft V4.1 Vector",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.08 }],
  },
  {
    id: "recraft/remove-background",
    displayName: "Recraft Remove Background",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.01 }],
  },
  {
    id: "recraft/replace-background",
    displayName: "Recraft Replace Background",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
];
