import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

/** Baseline Luma prices (mirrors litegen models/luma.yaml). All video. */
export const seed: SeedModel[] = [
  {
    id: 'luma/dream-machine',
    displayName: 'Luma Dream Machine',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.35 }],
  },
  {
    id: 'luma/ray-2',
    displayName: 'Luma Ray 2',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.4 }],
  },
  {
    id: 'luma/ray-3',
    displayName: 'Luma Ray 3',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.5 }],
  },
  {
    id: 'luma/ray-flash-2',
    displayName: 'Luma Ray Flash 2',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.2 }],
  },
  {
    id: 'luma/ray-hdr-3',
    displayName: 'Luma Ray HDR 3',
    mediaType: MediaType.VIDEO,
    prices: [{ unit: PriceUnit.PER_VIDEO, amountUsd: 0.65 }],
  },
];
