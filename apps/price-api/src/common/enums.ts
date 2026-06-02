/**
 * Shared domain enums. Kept framework-free so they can be imported by the
 * provider registry, the Nest modules, and the standalone coverage/seed scripts
 * alike.
 */

/** The kind of media a model produces. */
export enum MediaType {
  IMAGE = 'image',
  VIDEO = 'video',
}

/** How a provider's (or an individual model's) pricing is kept up to date. */
export enum ProviderMode {
  /** Refreshed automatically by a cron-scheduled scraper. */
  SCRAPED = 'scraped',
  /** Curated by hand; never overwritten by a scraper. */
  MANUAL = 'manual',
}

/**
 * The billing unit a price component is denominated in. Image models are
 * typically per-image; video models are per-second or per-video; some hosts
 * (e.g. GPU-time billing) are effectively per-request.
 */
export enum PriceUnit {
  PER_IMAGE = 'per_image',
  PER_VIDEO = 'per_video',
  PER_SECOND = 'per_second',
  PER_MEGAPIXEL = 'per_megapixel',
  PER_REQUEST = 'per_request',
}

/** Where the currently-served price value came from. */
export enum PriceSource {
  /** Written by a successful scrape run. */
  SCRAPED = 'scraped',
  /** Curated by hand (manual mode or admin upsert). */
  MANUAL = 'manual',
  /** Seeded baseline served because no fresher value is available. */
  FALLBACK = 'fallback',
}

/** Confidence in the currently-served price. */
export enum Freshness {
  /** Last refresh succeeded within tolerance. */
  FRESH = 'fresh',
  /** Last refresh failed but a previous good value is still being served. */
  STALE = 'stale',
  /** Repeated failures past the configured threshold. */
  FAILED = 'failed',
}

/** Outcome of a single scrape run for one provider. */
export enum ScrapeStatus {
  SUCCESS = 'success',
  /** Some models updated, others failed to parse. */
  PARTIAL = 'partial',
  FAILED = 'failed',
  /** Provider scraper is a stub (not yet implemented). */
  SKIPPED = 'skipped',
}

/** OAuth scopes recognised by the API. */
export enum Scope {
  READ = 'pricing:read',
  ADMIN = 'pricing:admin',
}

export const ALL_SCOPES: Scope[] = [Scope.READ, Scope.ADMIN];
