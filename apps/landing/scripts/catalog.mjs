/**
 * The repo's model catalog (`models/*.yaml`), read at build time by the sync
 * scripts and the tests. The Cloudflare Pages build has the monorepo checked
 * out but no gateway running, so the catalog is the source of every generated
 * snapshot. `mock.yaml` is a test fixture, not an integration, and is skipped.
 */
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { parse } from 'yaml';

export const MODELS_DIR = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'models');

/** Every model row of the shipped catalog (mock skipped). Throws when there is none. */
export function readCatalog(dir = MODELS_DIR) {
  if (!existsSync(dir)) throw new Error(`models dir not found at ${dir}`);
  const files = readdirSync(dir).filter((f) => f.endsWith('.yaml') && f !== 'mock.yaml');
  if (files.length === 0) throw new Error('no model yaml files found');
  const all = [];
  for (const f of files) {
    const doc = parse(readFileSync(join(dir, f), 'utf8'));
    for (const m of doc?.models ?? []) all.push(m);
  }
  if (all.length === 0) throw new Error('no models parsed');
  return all;
}
