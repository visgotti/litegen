/**
 * Copy the canonical OpenAPI spec (sdks/openapi.json — emitted by the gateway's
 * GET /openapi.json and committed for the SDKs) into the landing app's public/
 * dir so the static export serves it at /openapi.json for the embedded Redoc
 * reference page. Only an OpenAPI 3 document with paths is copied. When the
 * source is missing or is not one (an empty file after a failed fetch, an HTML
 * page), the build fails and public/ is left as it is; set
 * LITEGEN_SYNC_KEEP_COMMITTED=1 to build with the committed copy anyway (see
 * sync-failure.mjs).
 */
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { syncFailed } from './sync-failure.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, '..', '..', '..', 'sdks', 'openapi.json');
const dst = join(here, '..', 'public', 'openapi.json');

/** sdks/openapi.json's text, once it is known to be an OpenAPI 3 document with paths. */
function readSpec() {
  if (!existsSync(src)) throw new Error(`source not found: ${src}`);
  const text = readFileSync(src, 'utf8');
  let doc;
  try {
    doc = JSON.parse(text);
  } catch (e) {
    throw new Error(`${src} is not JSON (${e.message})`);
  }
  const paths = doc?.paths;
  if (typeof doc?.openapi !== 'string' || !doc.openapi.startsWith('3.') || !paths || typeof paths !== 'object' || Object.keys(paths).length === 0) {
    throw new Error(`${src} is not an OpenAPI 3 document with paths`);
  }
  return text;
}

try {
  writeFileSync(dst, readSpec());
  console.log('[sync-openapi] copied sdks/openapi.json -> public/openapi.json');
} catch (err) {
  syncFailed('sync-openapi', err instanceof Error ? err.message : String(err), 'keeping existing public/openapi.json');
}
