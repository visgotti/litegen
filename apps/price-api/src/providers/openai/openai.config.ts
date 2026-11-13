import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * OpenAI's GPT Image models are billed per token: a per-image price follows
 * from the output tokens a size and quality produce, which a pricing-table
 * scrape can't read. Their prices are curated by hand (manual mode), from the
 * token counts in the image-generation guide.
 */
export const config: ProviderConfig = {
  id: 'openai',
  displayName: 'OpenAI',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://developers.openai.com/api/docs/guides/image-generation',
  notes: 'GPT Image prices are per token; curated by hand as per-image prices of the default request.',
};
