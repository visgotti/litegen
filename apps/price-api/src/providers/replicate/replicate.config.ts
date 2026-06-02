import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: 'replicate',
  displayName: 'Replicate',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://replicate.com/pricing',
  notes: 'Manually curated; Replicate bills per-second of hardware, normalised to per-image/per-video estimates.',
};
