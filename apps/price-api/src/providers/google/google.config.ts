import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: 'google',
  displayName: 'Google',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://ai.google.dev/gemini-api/docs/pricing',
  notes: 'Manually curated from the Gemini API pricing docs.',
};
