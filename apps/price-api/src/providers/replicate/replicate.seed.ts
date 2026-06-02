import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline Replicate prices (mirrors litegen models/replicate.yaml). */
export const seed: SeedModel[] = [
  {
    id: 'replicate/flux-pro',
    displayName: 'Flux Pro',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.055 }],
  },
  {
    id: 'replicate/flux-dev',
    displayName: 'Flux Dev',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.025 }],
  },
  {
    id: 'replicate/flux-schnell',
    displayName: 'Flux Schnell',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.003 }],
  },
  {
    id: 'replicate/sdxl',
    displayName: 'SDXL on Replicate',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.012 }],
  },
  {
    id: 'replicate/sd3',
    displayName: 'SD3 on Replicate',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.035 }],
  },
  {
    id: 'replicate/video',
    displayName: 'Replicate Video (generic)',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.3 }],
  },
];
