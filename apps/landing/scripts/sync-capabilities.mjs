/**
 * Build-time sync: fetch GET /v1/models from a running gateway, derive the
 * per-provider packet vocabulary, and write capabilities.generated.ts.
 *
 * Fail-open: if the gateway is unreachable / returns nothing, keep the existing
 * committed snapshot (so `next build` never breaks offline). If no snapshot
 * exists yet, write the baked FALLBACK so imports always resolve.
 */
import { writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { deriveCapabilities, renderGeneratedTs, FALLBACK_CAPABILITIES } from './derive-capabilities.mjs';

const BASE = process.env.LITEGEN_API_URL ?? 'http://localhost:4000';
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '..', 'src', 'components', 'infra-flow', 'capabilities.generated.ts');

try {
  const res = await fetch(`${BASE}/v1/models`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const json = await res.json();
  const caps = deriveCapabilities(json.data ?? []);
  if (Object.keys(caps).length === 0) throw new Error('no models returned');
  writeFileSync(OUT, renderGeneratedTs(caps));
  console.log(`[sync-capabilities] wrote ${Object.keys(caps).length} providers from ${BASE}/v1/models`);
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  if (existsSync(OUT)) {
    console.warn(`[sync-capabilities] warning: ${msg}; keeping existing committed snapshot`);
    process.exit(0);
  }
  console.warn(`[sync-capabilities] warning: ${msg}; no snapshot exists, writing baked fallback`);
  writeFileSync(OUT, renderGeneratedTs(FALLBACK_CAPABILITIES));
}
