/**
 * Build-time sync: read the repo's models/*.yaml (the capability registry's
 * source of truth) and write src/config/models.generated.ts for the /reference
 * page's per-model capability table.
 *
 * Reads YAML directly (unlike sync-capabilities.mjs, which hits /v1/models)
 * because the Cloudflare Pages build has no gateway running and we need the full
 * per-model data. The monorepo is fully checked out there, so ../../../models
 * resolves. If the catalog can't be read, the build fails and the committed
 * snapshot is left as it is; set LITEGEN_SYNC_KEEP_COMMITTED=1 to build with it
 * anyway (see sync-failure.mjs).
 */
import { writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { deriveModels, renderGeneratedTs, renderProviderModelsTs } from './derive-models.mjs';
import { readCatalog } from './catalog.mjs';
import { syncFailed } from './sync-failure.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '..', 'src', 'config', 'models.generated.ts');
const OUT_PROVIDER = join(HERE, '..', 'src', 'components', 'infra-flow', 'provider-models.generated.ts');

try {
  // Skips the mock provider — it's a test fixture, not a real integration.
  const all = readCatalog();
  const models = deriveModels(all);
  writeFileSync(OUT, renderGeneratedTs(models));
  writeFileSync(OUT_PROVIDER, renderProviderModelsTs(models));
  console.log(`[sync-models] wrote ${models.length} models from models/*.yaml`);
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  if (existsSync(OUT)) {
    syncFailed('sync-models', msg, 'keeping existing committed snapshot');
  } else {
    syncFailed('sync-models', msg, 'no snapshot exists, writing empty list');
    writeFileSync(OUT, renderGeneratedTs([]));
  }
}
