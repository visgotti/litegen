// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "bedrock/luma.ray-v2:0",
    displayName: "Luma Ray 2 (Bedrock)",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 7.5 }],
  },
  {
    id: "bedrock/stability.sd3-5-large-v1:0",
    displayName: "Stable Diffusion 3.5 Large (Bedrock)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.08 }],
  },
  {
    id: "bedrock/stability.stable-image-core-v1:1",
    displayName: "Stable Image Core (Bedrock)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: "bedrock/stability.stable-image-ultra-v1:1",
    displayName: "Stable Image Ultra (Bedrock)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.14 }],
  },
];
