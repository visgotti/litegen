import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** MiniMax: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'minimax',
  displayName: 'MiniMax',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://platform.minimax.io/docs/pricing/overview.md',
  notes: 'Prices from MiniMax\'s pricing overview, curated by hand.',
};
