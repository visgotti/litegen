import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** PixVerse: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'pixverse',
  displayName: 'PixVerse',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://docs.platform.pixverse.ai/pixverse-api-pricing-2394995m0.md',
  notes: 'Credit prices from PixVerse\'s API pricing page, curated by hand.',
};
