import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: 'luma',
  displayName: 'Luma',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  // The old /dream-machine/api/pricing now redirects to the Agents app, with no
  // prices (2026-11-09). Ray 2's create costs are in its changelog:
  // https://docs.lumalabs.ai/changelog/upscale.md
  pricingUrl: 'https://lumalabs.ai/api',
  notes: 'Manually curated; per-video baseline (per-second tiers can be added).',
};
