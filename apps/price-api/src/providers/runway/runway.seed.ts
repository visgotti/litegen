import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline Runway prices (mirrors litegen models/runway.yaml). All video. */
export const seed: SeedModel[] = [
  {
    id: 'runway/gen-3',
    displayName: 'Runway Gen-3',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.5 }],
  },
  {
    id: 'runway/gen-3-turbo',
    displayName: 'Runway Gen-3 Turbo',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.25 }],
  },
];
