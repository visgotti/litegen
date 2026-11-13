import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Kling AI: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'kling',
  displayName: 'Kling AI',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://kling.ai/document-api/pricing/base/video.md',
  notes: 'Video prices from this page, image prices from .../pricing/base/image.md; curated by hand.',
};
