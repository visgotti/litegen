import { StubScraper } from '../base';

/**
 * Stub: Stability prices are manually curated for now. To make this provider
 * dynamic, replace with a `BaseProviderScraper` subclass that parses
 * `config.pricingUrl`, and flip `stability.config.ts` to SCRAPED + a cron.
 */
export const scraper = new StubScraper('stability');
