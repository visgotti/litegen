import {
  ProviderScraper,
  ScrapeContext,
  ScrapedPriceComponent,
  ScraperNotImplementedError,
} from './types';

const USER_AGENT = 'litegen-price-api/0.1 (+https://github.com/litegen)';

/**
 * Base class for real scrapers. Provides HTTP helpers with a hard timeout, a
 * descriptive user-agent, and status checks. Subclasses implement `scrape()`.
 */
export abstract class BaseProviderScraper implements ProviderScraper {
  abstract readonly providerId: string;
  readonly implemented: boolean = true;

  abstract scrape(ctx: ScrapeContext): Promise<ScrapedPriceComponent[]>;

  protected async fetchWithTimeout(
    url: string,
    ctx: ScrapeContext,
    accept: string,
  ): Promise<Response> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), ctx.timeoutMs);
    try {
      const res = await ctx.fetch(url, {
        signal: controller.signal,
        headers: { 'user-agent': USER_AGENT, accept },
        redirect: 'follow',
      });
      if (!res.ok) {
        throw new Error(`GET ${url} responded ${res.status}`);
      }
      return res;
    } finally {
      clearTimeout(timer);
    }
  }

  protected async fetchText(url: string, ctx: ScrapeContext): Promise<string> {
    const res = await this.fetchWithTimeout(url, ctx, 'text/html,application/xhtml+xml');
    return res.text();
  }

  protected async fetchJson<T>(url: string, ctx: ScrapeContext): Promise<T> {
    const res = await this.fetchWithTimeout(url, ctx, 'application/json');
    return (await res.json()) as T;
  }
}

/**
 * Placeholder scraper for a provider whose prices are curated by hand (or whose
 * live page cannot yet be parsed reliably). It always throws, which the scrape
 * service treats as "skipped — keep the existing manual/seeded price". This is
 * what lets every supported provider satisfy the coverage guarantee while only
 * a subset is genuinely scraped.
 */
export class StubScraper implements ProviderScraper {
  readonly implemented = false;
  constructor(public readonly providerId: string) {}

  async scrape(): Promise<ScrapedPriceComponent[]> {
    throw new ScraperNotImplementedError(this.providerId);
  }
}
