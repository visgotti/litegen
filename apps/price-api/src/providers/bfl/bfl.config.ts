import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Black Forest Labs: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'bfl',
  displayName: 'Black Forest Labs',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://docs.bfl.ml/quick_start/pricing.md',
  notes: 'FLUX prices from BFL\'s pricing page, curated by hand.',
};
