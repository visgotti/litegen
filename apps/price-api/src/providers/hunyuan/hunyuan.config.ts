import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/** Tencent Hunyuan: no scraper; every price is curated by hand (manual mode). */
export const config: ProviderConfig = {
  id: 'hunyuan',
  displayName: 'Tencent Hunyuan',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://cloud.tencent.com/document/product/1616/118994',
  notes: 'Credits priced in CNY, converted to USD in litegen\'s catalog; curated by hand.',
};
