import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Recraft: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'recraft',
  displayName: 'Recraft',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://www.recraft.ai/docs/api-reference/pricing.md',
  notes: 'Prices from Recraft\'s API pricing page, curated by hand.',
};
