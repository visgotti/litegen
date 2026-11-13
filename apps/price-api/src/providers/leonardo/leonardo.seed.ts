// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "leonardo/diffusion-xl",
    displayName: "Leonardo Diffusion XL",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.02 }],
  },
  {
    id: "leonardo/kling2.5",
    displayName: "Leonardo Kling 2.5 Turbo",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 1 }],
  },
  {
    id: "leonardo/motion2",
    displayName: "Leonardo Motion 2.0",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.3 }],
  },
  {
    id: "leonardo/veo3.1",
    displayName: "Leonardo Veo 3.1",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 2.5 }],
  },
  {
    id: "leonardo/veo3.1-fast",
    displayName: "Leonardo Veo 3.1 Fast",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 2.5 }],
  },
];
