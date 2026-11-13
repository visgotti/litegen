import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Meshy: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'meshy',
  displayName: 'Meshy',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://docs.meshy.ai/en/api/pricing',
  notes: 'Credit prices from Meshy\'s API pricing page, curated by hand.',
};
