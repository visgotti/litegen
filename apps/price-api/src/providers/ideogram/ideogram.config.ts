import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Ideogram: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'ideogram',
  displayName: 'Ideogram',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://ideogram.ai/pricing/?pricing_tab=api',
  notes: 'Prices from Ideogram\'s API pricing tab, curated by hand.',
};
