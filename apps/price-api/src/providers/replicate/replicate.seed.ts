// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "replicate/flux-1.1-pro",
    displayName: "Flux 1.1 Pro",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: "replicate/flux-3-image",
    displayName: "FLUX 3 Image (Replicate)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.024 }],
  },
  {
    id: "replicate/flux-dev",
    displayName: "Flux Dev",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.025 }],
  },
  {
    id: "replicate/flux-pro",
    displayName: "Flux Pro",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.055 }],
  },
  {
    id: "replicate/flux-schnell",
    displayName: "Flux Schnell",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.003 }],
  },
  {
    id: "replicate/sd3",
    displayName: "SD3 on Replicate",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.035 }],
  },
  {
    id: "replicate/sd3.5-large",
    displayName: "Stable Diffusion 3.5 Large (Replicate)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.065 }],
  },
  {
    id: "replicate/sd3.5-large-turbo",
    displayName: "Stable Diffusion 3.5 Large Turbo (Replicate)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: "replicate/sd3.5-medium",
    displayName: "Stable Diffusion 3.5 Medium (Replicate)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.035 }],
  },
  {
    id: "replicate/sdxl",
    displayName: "SDXL on Replicate",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.005 }],
  },
  {
    id: "replicate/video",
    displayName: "Replicate Video (generic)",
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.1 }],
  },
];
