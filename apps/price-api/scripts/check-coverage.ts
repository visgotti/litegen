/**
 * Coverage guarantee: every provider/model that litegen-core supports MUST have
 * at least a stubbed implementation here — a provider config, seed data, and a
 * registered scraper (real or stub). Run in CI; non-zero exit on any gap.
 *
 *   npm run coverage:check      # verify
 *   npm run coverage:fix        # scaffold missing provider folders
 *
 * The canonical model set is read from litegen-core's models/*.yaml at dev/CI
 * time only (LITEGEN_MODELS_DIR); there is no runtime dependency.
 */
import * as fs from 'fs';
import * as path from 'path';
import { loadEnv } from '../src/config/load-env';
import { PROVIDER_REGISTRY, getProviderDefinition } from '../src/providers/registry';
import { LitegenModel, readLitegenModels } from './lib/litegen-models';

loadEnv();

const FIX = process.argv.includes('--fix');
const modelsDir = process.env.LITEGEN_MODELS_DIR ?? '../../models';

function registryModelIds(): Set<string> {
  return new Set(PROVIDER_REGISTRY.flatMap((p) => p.seed.map((m) => m.id)));
}

interface Gap {
  providerMissing: string[]; // canonical providers with no registry entry
  modelsMissing: LitegenModel[]; // canonical models not seeded
}

function findGaps(canonical: LitegenModel[]): Gap {
  const seeded = registryModelIds();
  const modelsMissing = canonical.filter((m) => !seeded.has(m.id));
  const providersMissingSet = new Set<string>();
  for (const m of canonical) {
    if (!getProviderDefinition(m.provider)) {
      providersMissingSet.add(m.provider);
    }
  }
  return { providerMissing: [...providersMissingSet], modelsMissing };
}

function printSummary(canonical: LitegenModel[]): void {
  const byProvider = new Map<string, LitegenModel[]>();
  for (const m of canonical) {
    byProvider.set(m.provider, [...(byProvider.get(m.provider) ?? []), m]);
  }
  console.log('\nProvider coverage:');
  console.log('  provider      models  registry  scraper');
  console.log('  ------------  ------  --------  -----------------');
  for (const [provider, models] of [...byProvider.entries()].sort()) {
    const def = getProviderDefinition(provider);
    const inReg = def ? 'yes' : 'NO';
    const scraper = def
      ? def.scraper.implemented
        ? `real (${def.config.mode})`
        : `stub (${def.config.mode})`
      : '-';
    console.log(
      `  ${provider.padEnd(12)}  ${String(models.length).padStart(6)}  ${inReg.padStart(8)}  ${scraper}`,
    );
  }
}

function scaffoldProvider(provider: string, models: LitegenModel[]): void {
  const dir = path.resolve(__dirname, '..', 'src', 'providers', provider);
  fs.mkdirSync(dir, { recursive: true });

  const configTs = `import { ProviderMode } from '../../common/enums';
import { ProviderConfig } from '../types';

export const config: ProviderConfig = {
  id: '${provider}',
  displayName: '${provider}',
  mode: ProviderMode.MANUAL,
  cronSchedule: null,
  pricingUrl: null,
  notes: 'Auto-scaffolded stub — fill in display name, pricing URL, and prices.',
};
`;

  const seedEntries = models
    .map((m) => {
      const unit = m.mediaType === 'video' ? 'PriceUnit.PER_VIDEO' : 'PriceUnit.PER_IMAGE';
      const media = m.mediaType === 'video' ? 'MediaType.VIDEO' : 'MediaType.IMAGE';
      const amount = m.baseCostUsd ?? 0;
      return `  {
    id: '${m.id}',
    displayName: '${m.id}',
    mediaType: ${media},
    prices: [{ unit: ${unit}, amountUsd: ${amount} }],
  },`;
    })
    .join('\n');

  const seedTs = `import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
${seedEntries}
];
`;

  const scraperTs = `import { StubScraper } from '../base';

export const scraper = new StubScraper('${provider}');
`;

  fs.writeFileSync(path.join(dir, `${provider}.config.ts`), configTs);
  fs.writeFileSync(path.join(dir, `${provider}.seed.ts`), seedTs);
  fs.writeFileSync(path.join(dir, `${provider}.scraper.ts`), scraperTs);
  console.log(`  scaffolded src/providers/${provider}/`);
}

function main(): void {
  const canonical = readLitegenModels(modelsDir);
  console.log(`Canonical litegen models: ${canonical.length} across ${new Set(canonical.map((m) => m.provider)).size} providers (from ${modelsDir})`);

  let gaps = findGaps(canonical);

  if (FIX && gaps.providerMissing.length > 0) {
    console.log('\n--fix: scaffolding missing provider folders...');
    for (const provider of gaps.providerMissing) {
      scaffoldProvider(
        provider,
        canonical.filter((m) => m.provider === provider),
      );
    }
    console.log(
      '\nScaffolding done. Register the new provider(s) in src/providers/registry.ts, then re-run coverage:check.',
    );
    // Models within existing providers can't be auto-merged safely; re-evaluate.
    gaps = findGaps(canonical);
  }

  printSummary(canonical);

  const hardFailures: string[] = [];
  if (gaps.providerMissing.length > 0) {
    hardFailures.push(`Providers missing from registry: ${gaps.providerMissing.join(', ')}`);
  }
  if (gaps.modelsMissing.length > 0) {
    hardFailures.push(
      `Models missing a seeded implementation:\n    - ${gaps.modelsMissing.map((m) => m.id).join('\n    - ')}`,
    );
  }

  if (hardFailures.length > 0) {
    console.error('\n❌ Coverage check FAILED:');
    for (const f of hardFailures) {
      console.error(`  - ${f}`);
    }
    if (!FIX) {
      console.error('\n  Run `npm run coverage:fix` to scaffold missing provider folders.');
    }
    process.exit(1);
  }

  console.log(`\n✅ Coverage OK: all ${canonical.length} litegen models have a config, seed, and scraper.`);
}

main();
