import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Leonardo.Ai: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'leonardo',
  displayName: 'Leonardo.Ai',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: null,
  notes: 'Leonardo publishes API prices only in its logged-in calculator, so these are litegen\'s unverified catalog prices; curated by hand.',
};
