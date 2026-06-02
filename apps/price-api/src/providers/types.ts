import { MediaType, PriceUnit, ProviderMode } from '../common/enums';
import { Tier } from '../common/tier-key';

/**
 * Framework-free contracts shared by the provider registry, the Nest scraping
 * module, and the standalone coverage/seed scripts. Nothing here imports Nest.
 */

/** A single baseline price component declared in a provider's seed file. */
export interface SeedPrice {
  unit: PriceUnit;
  amountUsd: number;
  /** Denominator the price applies to (per N images/seconds). Default 1. */
  unitAmount?: number;
  /** ISO currency code. Default 'USD'. */
  currency?: string;
  /** Optional qualifiers, e.g. `{resolution: '1080p'}`. */
  tier?: Tier;
}

/** A model declared in a provider's seed file. */
export interface SeedModel {
  /** Fully-qualified id, e.g. `openai/dall-e-3`. */
  id: string;
  displayName: string;
  mediaType: MediaType;
  /** Override the provider default mode for this model only. */
  modeOverride?: ProviderMode | null;
  prices: SeedPrice[];
}

/** Per-provider configuration: refresh strategy + provenance. */
export interface ProviderConfig {
  /** Slug, e.g. `openai`. Must match the litegen provider id. */
  id: string;
  displayName: string;
  /** Default refresh mode for the provider's models. */
  mode: ProviderMode;
  /** node-cron expression for scheduled scrapes; null when not scraped. */
  cronSchedule?: string | null;
  /** Public pricing page / API the scraper reads. */
  pricingUrl?: string | null;
  notes?: string;
}

/** A normalised price observed by a scraper. */
export interface ScrapedPriceComponent {
  modelId: string;
  unit: PriceUnit;
  amountUsd: number;
  unitAmount?: number;
  currency?: string;
  tier?: Tier;
}

/** Runtime context handed to a scraper, with an injectable fetch for testing. */
export interface ScrapeContext {
  timeoutMs: number;
  fetch: typeof fetch;
}

/** Thrown by stub scrapers; treated as "no update, keep current price". */
export class ScraperNotImplementedError extends Error {
  constructor(public readonly providerId: string) {
    super(`Scraper for provider "${providerId}" is not implemented`);
    this.name = 'ScraperNotImplementedError';
  }
}

/** Contract every provider scraper implements (real or stub). */
export interface ProviderScraper {
  readonly providerId: string;
  /** False for stubs that have no real parsing yet. */
  readonly implemented: boolean;
  scrape(ctx: ScrapeContext): Promise<ScrapedPriceComponent[]>;
}

/** A fully-assembled provider: its config, seed data, and scraper. */
export interface ProviderDefinition {
  config: ProviderConfig;
  seed: SeedModel[];
  scraper: ProviderScraper;
}
