import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Tripo: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'tripo',
  displayName: 'Tripo',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://developers.tripo3d.ai/en/pricing',
  notes: 'Credit prices from Tripo\'s pricing page, curated by hand.',
};
