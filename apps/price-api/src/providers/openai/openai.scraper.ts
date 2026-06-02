import { PriceUnit } from '../../common/enums';
import { BaseProviderScraper } from '../base';
import { PriceAlias, parsePricingTables } from '../scrape-parsers';
import { ScrapeContext, ScrapedPriceComponent } from '../types';
import { config } from './openai.config';

/** Rows we look for on the OpenAI pricing page → our model ids. */
const ALIASES: PriceAlias[] = [
  { modelId: 'openai/dall-e-3', match: /dall.?e[\s·-]*3/i, unit: PriceUnit.PER_IMAGE },
  { modelId: 'openai/dall-e-2', match: /dall.?e[\s·-]*2/i, unit: PriceUnit.PER_IMAGE },
];

/**
 * Scrapes DALL-E image prices from the OpenAI pricing page. Sora is intentionally
 * not scraped (manual). Parsing is delegated to the generic table parser, which
 * is fixture-tested; on a page-structure change it returns fewer/no components
 * and the scrape service keeps the last-known-good values.
 */
export class OpenAiScraper extends BaseProviderScraper {
  readonly providerId = 'openai';

  async scrape(ctx: ScrapeContext): Promise<ScrapedPriceComponent[]> {
    const html = await this.fetchText(config.pricingUrl as string, ctx);
    return parsePricingTables(html, ALIASES);
  }
}
