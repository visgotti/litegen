import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: 'runway',
  displayName: 'Runway',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://runwayml.com/pricing',
  notes: 'Manually curated; credit-based pricing normalised to per-video USD.',
};
