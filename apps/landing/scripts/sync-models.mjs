/**
 * Build-time sync: read the repo's models/*.yaml (the capability registry's
 * source of truth) and write src/config/models.generated.ts for the /reference
 * page's per-model capability table.
 *
 * Reads YAML directly (unlike sync-capabilities.mjs, which hits /v1/models)
 * because the Cloudflare Pages build has no gateway running and we need the full
 * per-model data. The monorepo is fully checked out there, so ../../../models
 * resolves. Fail-open: keep the committed snapshot if anything goes wrong.
 */
import { readFileSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { parse } from 'yaml';
import { deriveModels, renderGeneratedTs, renderProviderModelsTs } from './derive-models.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const MODELS_DIR = join(HERE, '..', '..', '..', 'models');
const OUT = join(HERE, '..', 'src', 'config', 'models.generated.ts');
const OUT_PROVIDER = join(HERE, '..', 'src', 'components', 'infra-flow', 'provider-models.generated.ts');

try {
  if (!existsSync(MODELS_DIR)) throw new Error(`models dir not found at ${MODELS_DIR}`);
  // Skip the mock provider — it's a test fixture, not a real integration.
  const files = readdirSync(MODELS_DIR).filter(
    (f) => f.endsWith('.yaml') && f !== 'mock.yaml',
  );
  if (files.length === 0) throw new Error('no model yaml files found');

  const all = [];
  for (const f of files) {
    const doc = parse(readFileSync(join(MODELS_DIR, f), 'utf8'));
    for (const m of doc?.models ?? []) all.push(m);
  }
  if (all.length === 0) throw new Error('no models parsed');

  const models = deriveModels(all);
  writeFileSync(OUT, renderGeneratedTs(models));
  writeFileSync(OUT_PROVIDER, renderProviderModelsTs(models));
  console.log(`[sync-models] wrote ${models.length} models from ${files.length} provider files`);
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  if (existsSync(OUT)) {
    console.warn(`[sync-models] warning: ${msg}; keeping existing committed snapshot`);
    process.exit(0);
  }
  console.warn(`[sync-models] warning: ${msg}; no snapshot exists, writing empty list`);
  writeFileSync(OUT, renderGeneratedTs([]));
}
