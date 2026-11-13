import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: 'runway',
  displayName: 'Runway',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  // API credit costs: "for $0.01 per credit", per model (2026-11-09). The old
  // runwayml.com/pricing is the consumer plans page.
  pricingUrl: 'https://docs.dev.runwayml.com/guides/pricing',
  notes: 'Manually curated; credit-based pricing normalised to per-video USD.',
};
