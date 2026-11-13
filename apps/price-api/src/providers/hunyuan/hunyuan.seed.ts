// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "hunyuan/hunyuan-image",
    displayName: "Hunyuan Image",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.0745 }],
  },
  {
    id: "hunyuan/hunyuan-video",
    displayName: "Hunyuan Video",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.268 }],
  },
  {
    id: "hunyuan/hunyuan-video-i2v",
    displayName: "Hunyuan Video I2V",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.894 }],
  },
];
