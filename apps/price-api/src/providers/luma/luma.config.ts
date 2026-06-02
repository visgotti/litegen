import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: 'luma',
  displayName: 'Luma',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://lumalabs.ai/dream-machine/api/pricing',
  notes: 'Manually curated; per-video baseline (per-second tiers can be added).',
};
