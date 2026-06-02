import { PriceUnit } from '../../common/enums';
import { BaseProviderScraper } from '../base';
import { PriceAlias, parsePricingTables } from '../scrape-parsers';
import { ScrapeContext, ScrapedPriceComponent } from '../types';
import { config } from './fal.config';

const ALIASES: PriceAlias[] = [
  { modelId: 'fal/flux-pro', match: /flux\.?1?\s*\[?pro\]?/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'fal/flux-dev', match: /flux\.?1?\s*\[?dev\]?/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'fal/flux-schnell', match: /flux\.?1?\s*\[?schnell\]?/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'fal/sdxl', match: /sdxl/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'fal/sd35-medium', match: /sd\s*3\.5\s*medium/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'fal/recraft-v3', match: /recraft\s*v?3/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'fal/auraflow', match: /auraflow/i, unit: PriceUnit.PER_IMAGE },
];

/**
 * Scrapes Fal image-model prices from the public pricing page. The generic
 * video endpoint is excluded (manual). Resilient to markup changes via the
 * fixture-tested generic table parser.
 */
export class FalScraper extends BaseProviderScraper {
  readonly providerId = 'fal';

  async scrape(ctx: ScrapeContext): Promise<ScrapedPriceComponent[]> {
    const html = await this.fetchText(config.pricingUrl as string, ctx);
    return parsePricingTables(html, ALIASES);
  }
}
