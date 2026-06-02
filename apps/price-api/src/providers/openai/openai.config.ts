import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * OpenAI is scraped daily from the public API pricing page. `sora` is pinned to
 * manual mode (see seed) because its video pricing is not a simple per-call rate
 * and is not reliably parseable from the pricing table — demonstrating a
 * per-model override of the provider default.
 */
export const config: ProviderConfig = {
  id: 'openai',
  displayName: 'OpenAI',
  mode: ProviderMode.SCRAPED,
  cronSchedule: '0 6 * * *',
  pricingUrl: 'https://openai.com/api/pricing/',
  notes: 'Image models scraped daily; Sora is manually curated.',
};
