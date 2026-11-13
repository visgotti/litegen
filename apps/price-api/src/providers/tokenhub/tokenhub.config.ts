import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

/**
 * Tencent TokenHub: no scraper; prices are curated by hand from its price
 * page (CNY per million tokens at the page's per-output token counts,
 * converted to USD in models/tokenhub.yaml).
 */
export const config: ProviderConfig = {
  id: 'tokenhub',
  displayName: 'Tencent TokenHub',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: 'https://cloud.tencent.com/document/product/1823/130055',
  notes: 'Token-billed reference prices from TokenHub\'s 模型价格 page (CNY), curated by hand.',
};
