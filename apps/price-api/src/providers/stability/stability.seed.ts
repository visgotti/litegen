// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "stability/core",
    displayName: "Stable Image Core",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.03 }],
  },
  {
    id: "stability/inpaint",
    displayName: "Stable Image Inpaint",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.05 }],
  },
  {
    id: "stability/outpaint",
    displayName: "Stable Image Outpaint",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: "stability/remove-background",
    displayName: "Stable Image Remove Background",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.05 }],
  },
  {
    id: "stability/sd3-large",
    displayName: "Stable Diffusion 3.5 Large",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.065 }],
  },
  {
    id: "stability/sd3-turbo",
    displayName: "Stable Diffusion 3.5 Large Turbo",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: "stability/sd3.5-medium",
    displayName: "Stable Diffusion 3.5 Medium",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.035 }],
  },
  {
    id: "stability/sdxl",
    displayName: "Stable Diffusion XL",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.009 }],
  },
  {
    id: "stability/search-and-replace",
    displayName: "Stable Image Search and Replace",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.05 }],
  },
  {
    id: "stability/ultra",
    displayName: "Stable Image Ultra",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.08 }],
  },
];
