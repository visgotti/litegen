import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** PixelLab: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'pixellab',
  displayName: 'PixelLab',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://api.pixellab.ai/v2/llms.txt',
  notes: 'Each PixelLab response reports its own USD cost; the seed is the catalog\'s default-request price.',
};
