// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "kling/kling-v2-1",
    displayName: "Kling Image v2.1",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.014 }],
  },
  {
    id: "kling/kling-v3",
    displayName: "Kling Image v3",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.028 }],
  },
  {
    id: "kling/video-kling-v2-5-turbo",
    displayName: "Kling Video v2.5 Turbo",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.21 }],
  },
  {
    id: "kling/video-kling-v2-6",
    displayName: "Kling Video v2.6",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.21 }],
  },
  {
    id: "kling/video-kling-v3",
    displayName: "Kling Video v3",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.42 }],
  },
  {
    id: "kling/video-kling-v3-turbo",
    displayName: "Kling Video v3 Turbo",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.56 }],
  },
  {
    id: "kling/video-kling-v4",
    displayName: "Kling Video v4",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 2.35 }],
  },
];
