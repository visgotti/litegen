/**
 * Build-time sync: derive the per-provider packet vocabulary from the repo's
 * models/*.yaml (read the way GET /v1/models projects each row) and write
 * capabilities.generated.ts. The catalog, not a running gateway, is the source:
 * the Cloudflare Pages build has none, and a dev gateway would add its mock
 * provider and runtime models.
 *
 * If the catalog can't be read, the build fails and the committed snapshot is
 * left as it is. With LITEGEN_SYNC_KEEP_COMMITTED=1 (see sync-failure.mjs) it
 * warns and builds with the committed snapshot instead, or, if none exists
 * yet, writes an empty one so imports always resolve.
 */
import { writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { capabilitiesFromCatalog, renderGeneratedTs } from './derive-capabilities.mjs';
import { readCatalog } from './catalog.mjs';
import { syncFailed } from './sync-failure.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '..', 'src', 'components', 'infra-flow', 'capabilities.generated.ts');

try {
  const caps = capabilitiesFromCatalog(readCatalog());
  if (Object.keys(caps).length === 0) throw new Error('no providers in the catalog');
  writeFileSync(OUT, renderGeneratedTs(caps));
  console.log(`[sync-capabilities] wrote ${Object.keys(caps).length} providers from models/*.yaml`);
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  if (existsSync(OUT)) {
    syncFailed('sync-capabilities', msg, 'keeping existing committed snapshot');
  } else {
    syncFailed('sync-capabilities', msg, 'no snapshot exists, writing an empty one');
    writeFileSync(OUT, renderGeneratedTs({}));
  }
}
