import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline Google prices (mirrors litegen models/google.yaml). */
export const seed: SeedModel[] = [
  {
    id: 'google/imagen-3',
    displayName: 'Imagen 3',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: 'google/gemini-2.5-flash-image',
    displayName: 'Gemini 2.5 Flash (image)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.02 }],
  },
  {
    id: 'google/gemini-3-pro-image',
    displayName: 'Gemini 3 Pro (image)',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.05 }],
  },
];
