import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * Fal's classic image models are scraped daily from the public pricing page
 * (the aliases in `fal.scraper.ts`). Every other fal model is curated by hand:
 * the registry pins what the scraper doesn't report to manual, e.g.
 * `fal/video`, which always runs fal's LTX Video endpoints (`fal-ai/ltx-video`
 * and its image-to-video twin) at their flat "$0.02 per videos".
 */
export const config: ProviderConfig = {
  id: 'fal',
  displayName: 'Fal',
  mode: ProviderMode.SCRAPED,
  cronSchedule: '30 6 * * *',
  pricingUrl: 'https://fal.ai/pricing',
  notes: 'Classic image models scraped daily; every other model manually curated.',
};
