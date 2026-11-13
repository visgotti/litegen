import { ProviderMode } from '../common/enums';
import { ProviderConfig, ProviderDefinition, ProviderScraper, SeedModel } from './types';

import { config as openaiConfig } from './openai/openai.config';
import { seed as openaiSeed } from './openai/openai.seed';
import { scraper as openaiScraper } from './openai/openai.scraper';

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

import { config as bflConfig } from './bfl/bfl.config';
import { seed as bflSeed } from './bfl/bfl.seed';
import { scraper as bflScraper } from './bfl/bfl.scraper';

import { config as bytedanceConfig } from './bytedance/bytedance.config';
import { seed as bytedanceSeed } from './bytedance/bytedance.seed';
import { scraper as bytedanceScraper } from './bytedance/bytedance.scraper';

import { config as hunyuanConfig } from './hunyuan/hunyuan.config';
import { seed as hunyuanSeed } from './hunyuan/hunyuan.seed';
import { scraper as hunyuanScraper } from './hunyuan/hunyuan.scraper';

import { config as bedrockConfig } from './bedrock/bedrock.config';
import { seed as bedrockSeed } from './bedrock/bedrock.seed';
import { scraper as bedrockScraper } from './bedrock/bedrock.scraper';

import { config as tokenhubConfig } from './tokenhub/tokenhub.config';
import { seed as tokenhubSeed } from './tokenhub/tokenhub.seed';
import { scraper as tokenhubScraper } from './tokenhub/tokenhub.scraper';

import { config as ideogramConfig } from './ideogram/ideogram.config';
import { seed as ideogramSeed } from './ideogram/ideogram.seed';
import { scraper as ideogramScraper } from './ideogram/ideogram.scraper';

import { config as klingConfig } from './kling/kling.config';
import { seed as klingSeed } from './kling/kling.seed';
import { scraper as klingScraper } from './kling/kling.scraper';

import { config as leonardoConfig } from './leonardo/leonardo.config';
import { seed as leonardoSeed } from './leonardo/leonardo.seed';
import { scraper as leonardoScraper } from './leonardo/leonardo.scraper';

import { config as meshyConfig } from './meshy/meshy.config';
import { seed as meshySeed } from './meshy/meshy.seed';
import { scraper as meshyScraper } from './meshy/meshy.scraper';

import { config as minimaxConfig } from './minimax/minimax.config';
import { seed as minimaxSeed } from './minimax/minimax.seed';
import { scraper as minimaxScraper } from './minimax/minimax.scraper';

import { config as pixellabConfig } from './pixellab/pixellab.config';
import { seed as pixellabSeed } from './pixellab/pixellab.seed';
import { scraper as pixellabScraper } from './pixellab/pixellab.scraper';

import { config as pixverseConfig } from './pixverse/pixverse.config';
import { seed as pixverseSeed } from './pixverse/pixverse.seed';
import { scraper as pixverseScraper } from './pixverse/pixverse.scraper';

import { config as recraftConfig } from './recraft/recraft.config';
import { seed as recraftSeed } from './recraft/recraft.seed';
import { scraper as recraftScraper } from './recraft/recraft.scraper';

import { config as tripoConfig } from './tripo/tripo.config';
import { seed as tripoSeed } from './tripo/tripo.seed';
import { scraper as tripoScraper } from './tripo/tripo.scraper';

import { config as viduConfig } from './vidu/vidu.config';
import { seed as viduSeed } from './vidu/vidu.seed';
import { scraper as viduScraper } from './vidu/vidu.scraper';

/**
 * A provider's definition, with every model its scraper doesn't report pinned
 * to manual: a scrape only refreshes what it reads, and would otherwise mark
 * the rest stale on every run.
 */
function define(config: ProviderConfig, seed: SeedModel[], scraper: ProviderScraper): ProviderDefinition {
  const scraped = new Set(scraper.modelIds);
  return {
    config,
    scraper,
    seed: seed.map((m) =>
      scraper.implemented && !scraped.has(m.id) && m.modeOverride == null
        ? { ...m, modeOverride: ProviderMode.MANUAL }
        : m,
    ),
  };
}

/**
 * The single source of truth for every provider the price-api knows about:
 * one per provider in litegen's catalog. Add a provider by creating
 * `src/providers/<id>/{config,seed,scraper}.ts` (`npm run coverage:fix`
 * scaffolds them and writes the seed) and appending it here. The seed service,
 * scraping module, and coverage script all read from this registry — keeping
 * them in lockstep automatically.
 */
export const PROVIDER_REGISTRY: ProviderDefinition[] = [
  define(openaiConfig, openaiSeed, openaiScraper),
  define(falConfig, falSeed, new FalScraper()),
  define(stabilityConfig, stabilitySeed, stabilityScraper),
  define(replicateConfig, replicateSeed, replicateScraper),
  define(googleConfig, googleSeed, googleScraper),
  define(lumaConfig, lumaSeed, lumaScraper),
  define(runwayConfig, runwaySeed, runwayScraper),
  define(bflConfig, bflSeed, bflScraper),
  define(bytedanceConfig, bytedanceSeed, bytedanceScraper),
  define(hunyuanConfig, hunyuanSeed, hunyuanScraper),
  define(tokenhubConfig, tokenhubSeed, tokenhubScraper),
  define(bedrockConfig, bedrockSeed, bedrockScraper),
  define(ideogramConfig, ideogramSeed, ideogramScraper),
  define(klingConfig, klingSeed, klingScraper),
  define(leonardoConfig, leonardoSeed, leonardoScraper),
  define(meshyConfig, meshySeed, meshyScraper),
  define(minimaxConfig, minimaxSeed, minimaxScraper),
  define(pixellabConfig, pixellabSeed, pixellabScraper),
  define(pixverseConfig, pixverseSeed, pixverseScraper),
  define(recraftConfig, recraftSeed, recraftScraper),
  define(tripoConfig, tripoSeed, tripoScraper),
  define(viduConfig, viduSeed, viduScraper),
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
