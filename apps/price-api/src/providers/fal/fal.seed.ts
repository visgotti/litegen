import { MediaType, PriceUnit, ProviderMode } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline Fal prices (mirrors litegen models/fal.yaml). */
export const seed: SeedModel[] = [
  {
    id: 'fal/flux-pro',
    displayName: 'Flux Pro (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.05 }],
  },
  {
    id: 'fal/flux-dev',
    displayName: 'Flux Dev (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.025 }],
  },
  {
    id: 'fal/flux-schnell',
    displayName: 'Flux Schnell (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.003 }],
  },
  {
    id: 'fal/sdxl',
    displayName: 'SDXL (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.015 }],
  },
  {
    id: 'fal/sd35-medium',
    displayName: 'SD 3.5 Medium (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.025 }],
  },
  {
    id: 'fal/recraft-v3',
    displayName: 'Recraft V3 (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: 'fal/auraflow',
    displayName: 'AuraFlow (Fal)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.02 }],
  },
  {
    id: 'fal/video',
    displayName: 'Fal Video (generic)',
    mediaType: MediaType.VIDEO,
    modeOverride: ProviderMode.MANUAL,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.2 }],
  },
];
