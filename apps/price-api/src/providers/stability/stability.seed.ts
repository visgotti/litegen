import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline Stability prices (mirrors litegen models/stability.yaml). */
export const seed: SeedModel[] = [
  {
    id: 'stability/sd3-large',
    displayName: 'Stable Diffusion 3 Large',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.065 }],
  },
  {
    id: 'stability/sd3-turbo',
    displayName: 'Stable Diffusion 3 Turbo',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: 'stability/core',
    displayName: 'Stable Image Core',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.03 }],
  },
  {
    id: 'stability/ultra',
    displayName: 'Stable Image Ultra',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.08 }],
  },
  {
    id: 'stability/sdxl',
    displayName: 'Stable Diffusion XL',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.002 }],
  },
];
