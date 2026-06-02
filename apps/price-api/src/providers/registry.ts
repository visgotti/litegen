import { ProviderDefinition, SeedModel } from './types';

import { config as openaiConfig } from './openai/openai.config';
import { seed as openaiSeed } from './openai/openai.seed';
import { OpenAiScraper } from './openai/openai.scraper';

import { config as falConfig } from './fal/fal.config';
import { seed as falSeed } from './fal/fal.seed';
import { FalScraper } from './fal/fal.scraper';

import { config as stabilityConfig } from './stability/stability.config';
import { seed as stabilitySeed } from './stability/stability.seed';
import { scraper as stabilityScraper } from './stability/stability.scraper';

import { config as replicateConfig } from './replicate/replicate.config';
import { seed as replicateSeed } from './replicate/replicate.seed';
import { scraper as replicateScraper } from './replicate/replicate.scraper';

import { config as googleConfig } from './google/google.config';
import { seed as googleSeed } from './google/google.seed';
import { scraper as googleScraper } from './google/google.scraper';

import { config as lumaConfig } from './luma/luma.config';
import { seed as lumaSeed } from './luma/luma.seed';
import { scraper as lumaScraper } from './luma/luma.scraper';

import { config as runwayConfig } from './runway/runway.config';
import { seed as runwaySeed } from './runway/runway.seed';
import { scraper as runwayScraper } from './runway/runway.scraper';

/**
 * The single source of truth for every provider the price-api knows about.
 * Add a provider by creating `src/providers/<id>/{config,seed,scraper}.ts` and
 * appending it here. The seed service, scraping module, and coverage script all
 * read from this registry — keeping them in lockstep automatically.
 */
export const PROVIDER_REGISTRY: ProviderDefinition[] = [
  { config: openaiConfig, seed: openaiSeed, scraper: new OpenAiScraper() },
  { config: falConfig, seed: falSeed, scraper: new FalScraper() },
  { config: stabilityConfig, seed: stabilitySeed, scraper: stabilityScraper },
  { config: replicateConfig, seed: replicateSeed, scraper: replicateScraper },
  { config: googleConfig, seed: googleSeed, scraper: googleScraper },
  { config: lumaConfig, seed: lumaSeed, scraper: lumaScraper },
  { config: runwayConfig, seed: runwaySeed, scraper: runwayScraper },
];

export function getProviderDefinition(id: string): ProviderDefinition | undefined {
  return PROVIDER_REGISTRY.find((p) => p.config.id === id);
}

/** Flattened list of every seeded model across all providers. */
export function allSeedModels(): SeedModel[] {
  return PROVIDER_REGISTRY.flatMap((p) => p.seed);
}

/** Set of every model id the registry declares. */
export function allModelIds(): Set<string> {
  return new Set(allSeedModels().map((m) => m.id));
}
