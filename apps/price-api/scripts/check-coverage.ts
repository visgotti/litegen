/**
 * Coverage guarantee: the registry mirrors litegen's catalog. Every provider
 * and model litegen-core carries MUST have a provider config, seed data and a
 * registered scraper (real or stub), and nothing the catalog dropped may stay
 * seeded. Run in CI; non-zero exit on any gap.
 *
 *   npm run coverage:check      # verify
 *   npm run coverage:fix        # scaffold missing providers, regenerate every seed
 *
 * The canonical model set is read from litegen-core's models/*.yaml at dev/CI
 * time only (LITEGEN_MODELS_DIR); there is no runtime dependency.
 */
import * as fs from 'fs';
import * as path from 'path';
import { loadEnv } from '../src/config/load-env';
import { PROVIDER_REGISTRY, getProviderDefinition } from '../src/providers/registry';
import { findGaps } from './lib/coverage';
import { LitegenModel, readLitegenModels } from './lib/litegen-models';
import { renderSeed } from './lib/seed-render';

loadEnv();

const FIX = process.argv.includes('--fix');
const modelsDir = process.env.LITEGEN_MODELS_DIR ?? '../../models';

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

/** A manual-mode config and a stub scraper for a provider the registry lacks. */
function scaffoldProvider(provider: string): void {
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
  notes: 'Auto-scaffolded stub — fill in the display name and pricing URL.',
};
`;

  const scraperTs = `import { StubScraper } from '../base';

export const scraper = new StubScraper('${provider}');
`;

  fs.writeFileSync(path.join(dir, `${provider}.config.ts`), configTs);
  fs.writeFileSync(path.join(dir, `${provider}.scraper.ts`), scraperTs);
  console.log(`  scaffolded src/providers/${provider}/`);
}

function writeSeed(provider: string, models: LitegenModel[]): void {
  const dir = path.resolve(__dirname, '..', 'src', 'providers', provider);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, `${provider}.seed.ts`), renderSeed(models));
}

function main(): void {
  const canonical = readLitegenModels(modelsDir);
  console.log(`Canonical litegen models: ${canonical.length} across ${new Set(canonical.map((m) => m.provider)).size} providers (from ${modelsDir})`);

  const gaps = findGaps(canonical, PROVIDER_REGISTRY);

  if (FIX) {
    if (gaps.providerMissing.length > 0) {
      console.log('\n--fix: scaffolding missing provider folders...');
      for (const provider of gaps.providerMissing) {
        scaffoldProvider(provider);
      }
      console.log('\nRegister the new provider(s) in src/providers/registry.ts, then re-run coverage:check.');
    }
    console.log("\n--fix: regenerating every provider's seed from the catalog...");
    for (const provider of new Set(canonical.map((m) => m.provider))) {
      writeSeed(provider, canonical.filter((m) => m.provider === provider));
    }
    // The registry was loaded before the rewrite; re-run coverage:check to
    // see the regenerated seeds.
    return;
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
  if (gaps.modelsRetired.length > 0) {
    hardFailures.push(
      `Seeded models the catalog no longer carries:\n    - ${gaps.modelsRetired.join('\n    - ')}`,
    );
  }

  if (hardFailures.length > 0) {
    console.error('\n❌ Coverage check FAILED:');
    for (const f of hardFailures) {
      console.error(`  - ${f}`);
    }
    console.error('\n  Run `npm run coverage:fix` to scaffold missing providers and regenerate the seeds.');
    process.exit(1);
  }

  console.log(`\n✅ Coverage OK: all ${canonical.length} litegen models have a config, seed, and scraper, and nothing else is seeded.`);
}

main();
