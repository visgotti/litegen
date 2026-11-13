import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * Amazon Bedrock: no scraper; the Stability AI per-image prices are curated by
 * hand (AWS publishes them through its metered unit map, which the pricing
 * page renders client-side).
 */
export const config: ProviderConfig = {
  id: 'bedrock',
  displayName: 'Amazon Bedrock',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://aws.amazon.com/bedrock/pricing/',
  notes: 'Stability AI per-image prices (us-west-2) from the Bedrock pricing page, curated by hand.',
};
