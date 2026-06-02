import { MediaType, PriceUnit, ProviderMode } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline OpenAI prices (mirrors litegen models/openai.yaml). */
export const seed: SeedModel[] = [
  {
    id: 'openai/dall-e-3',
    displayName: 'DALL-E 3',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.04 }],
  },
  {
    id: 'openai/dall-e-2',
    displayName: 'DALL-E 2',
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.02 }],
  },
  {
    id: 'openai/sora',
    displayName: 'Sora',
    mediaType: MediaType.VIDEO,
    modeOverride: ProviderMode.MANUAL,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.5 }],
  },
];
