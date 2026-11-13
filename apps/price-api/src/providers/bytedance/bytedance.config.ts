import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** ByteDance (BytePlus ModelArk): no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'bytedance',
  displayName: 'ByteDance (BytePlus ModelArk)',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://docs.byteplus.com/en/docs/modelark/model-pricing',
  notes: 'Seedream and Seedance prices from ModelArk\'s price tables, curated by hand.',
};
