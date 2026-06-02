/**
 * Copy the canonical OpenAPI spec (sdks/openapi.json — emitted by the gateway's
 * GET /openapi.json and committed for the SDKs) into the landing app's public/
 * dir so the static export serves it at /openapi.json for the embedded Redoc
 * reference page. Fail-open: if the source is missing, keep whatever is already
 * in public/ so the build never breaks.
 */
import { copyFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, '..', '..', '..', 'sdks', 'openapi.json');
const dst = join(here, '..', 'public', 'openapi.json');

try {
  if (!existsSync(src)) throw new Error(`source not found: ${src}`);
  copyFileSync(src, dst);
  console.log('[sync-openapi] copied sdks/openapi.json -> public/openapi.json');
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  console.warn(`[sync-openapi] warning: ${msg}; keeping existing public/openapi.json`);
  process.exit(0);
}
