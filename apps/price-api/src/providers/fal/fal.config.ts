import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * Fal image models are scraped daily from the public pricing page. The generic
 * `fal/video` endpoint is pinned to manual (see seed) because its price depends
 * on the underlying model selected at request time.
 */
export const config: ProviderConfig = {
  id: 'fal',
  displayName: 'Fal',
  mode: ProviderMode.SCRAPED,
  cronSchedule: '30 6 * * *',
  pricingUrl: 'https://fal.ai/pricing',
  notes: 'Image models scraped daily; generic video endpoint manually curated.',
};
