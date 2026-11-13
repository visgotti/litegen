// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "minimax/image-01",
    displayName: "MiniMax image-01",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.0035 }],
  },
  {
    id: "minimax/MiniMax-H3",
    displayName: "MiniMax H3",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.4 }],
  },
  {
    id: "minimax/MiniMax-H3-Highspeed",
    displayName: "MiniMax H3 Highspeed",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.3 }],
  },
  {
    id: "minimax/MiniMax-H3-Lite",
    displayName: "MiniMax H3 Lite",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.2 }],
  },
  {
    id: "minimax/MiniMax-H3-Max",
    displayName: "MiniMax H3 Max",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.4 }],
  },
  {
    id: "minimax/MiniMax-Hailuo-02",
    displayName: "MiniMax Hailuo 02",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.28 }],
  },
  {
    id: "minimax/MiniMax-Hailuo-2.3",
    displayName: "MiniMax Hailuo 2.3",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.28 }],
  },
  {
    id: "minimax/MiniMax-Hailuo-2.3-Fast",
    displayName: "MiniMax Hailuo 2.3 Fast",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.19 }],
  },
  {
    id: "minimax/S2V-01",
    displayName: "MiniMax S2V-01",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.65 }],
  },
  {
    id: "minimax/T2V-01-Director",
    displayName: "MiniMax T2V-01 Director",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.43 }],
  },
];
