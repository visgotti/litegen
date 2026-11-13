import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Vidu: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'vidu',
  displayName: 'Vidu',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://platform.vidu.com/docs/pricing.md',
  notes: 'Credit prices from Vidu\'s pricing page, curated by hand.',
};
