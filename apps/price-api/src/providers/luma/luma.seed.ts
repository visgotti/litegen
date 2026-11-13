// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "luma/dream-machine",
    displayName: "Luma Dream Machine",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.35 }],
  },
  {
    id: "luma/photon-1",
    displayName: "Luma Photon",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.015 }],
  },
  {
    id: "luma/photon-flash-1",
    displayName: "Luma Photon Flash",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.002 }],
  },
  {
    id: "luma/ray-2",
    displayName: "Luma Ray 2",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.4 }],
  },
  {
    id: "luma/ray-flash-2",
    displayName: "Luma Ray Flash 2",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.2 }],
  },
];
