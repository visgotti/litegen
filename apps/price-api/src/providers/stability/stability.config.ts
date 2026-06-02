import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * Stability prices are curated by hand (manual mode). `pricingUrl` is recorded
 * for provenance and as the target for a future real scraper; swap
 * `stability.scraper.ts` from a StubScraper to a BaseProviderScraper and set
 * `mode: SCRAPED` + a `cronSchedule` to enable dynamic refresh.
 */
export const config: ProviderConfig = {
  id: 'stability',
  displayName: 'Stability AI',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://platform.stability.ai/pricing',
  notes: 'Manually curated; credit-based pricing converted to per-image USD.',
};
