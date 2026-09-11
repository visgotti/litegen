// Definitions are the per-vendor source configs in scripts/model-drift/providers/.
// The real-data test here is the gate that fails when someone adds a model to
// models/*.yaml without teaching the drift check how to find it upstream.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadRegistry, loadDefinitions, validateDefinitions, formatReport } from '../lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = join(here, '..', '..', '..');

function defsDir(files) {
  const dir = mkdtempSync(join(tmpdir(), 'drift-defs-'));
  for (const [name, src] of Object.entries(files)) writeFileSync(join(dir, name), src);
  return dir;
}

test('loadDefinitions imports every provider module in the directory, sorted by provider', async () => {
  const dir = defsDir({
    'zeta.mjs': `export default { provider: 'zeta', models: {}, sources: [{ key: 'a', url: 'https://z.test', expect: 'text', extract: /z/g }] };`,
    'acme.mjs': `export default { provider: 'acme', models: {}, sources: [{ key: 'a', url: 'https://a.test', expect: 'text', extract: /a/g }] };`,
  });
  const defs = await loadDefinitions(dir);
  assert.deepEqual(defs.map((d) => d.provider), ['acme', 'zeta']);
});

test('loadDefinitions rejects a definition whose file name and provider disagree', async () => {
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acne', models: {}, sources: [{ key: 'a', url: 'https://a.test', expect: 'text', extract: /a/g }] };`,
  });
  await assert.rejects(loadDefinitions(dir), /acme\.mjs.*acne/);
});

test('loadDefinitions rejects a definition with no sources', async () => {
  const dir = defsDir({ 'acme.mjs': `export default { provider: 'acme', models: {}, sources: [] };` });
  await assert.rejects(loadDefinitions(dir), /acme\.mjs.*sources/);
});

test('loadDefinitions rejects duplicate source keys, which would overwrite each other\'s snapshot', async () => {
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acme', models: {}, sources: [
      { key: 'a', url: 'https://a.test/1', expect: 'text', extract: /a/g },
      { key: 'a', url: 'https://a.test/2', expect: 'text', extract: /a/g },
    ] };`,
  });
  await assert.rejects(loadDefinitions(dir), /acme\.mjs.*duplicate source key "a"/);
});

test('loadDefinitions accepts a discover source in place of a fixed url', async () => {
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acme', models: {}, sources: [{ key: 'a', discover: { url: 'https://a.test/docs', pattern: /x/ }, expect: 'text', extract: /a/g }] };`,
  });
  assert.equal((await loadDefinitions(dir)).length, 1);
});

test('loadDefinitions rejects an unknown expect kind', async () => {
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acme', models: {}, sources: [{ key: 'a', url: 'https://a.test', expect: 'yaml', extract: /a/g }] };`,
  });
  await assert.rejects(loadDefinitions(dir), /acme\.mjs.*expect/);
});

test('every model in models/*.yaml is mapped by exactly one in-sync drift definition', async () => {
  const registry = loadRegistry(join(REPO, 'models'));
  const defs = await loadDefinitions(join(REPO, 'scripts', 'model-drift', 'providers'));
  assert.deepEqual(validateDefinitions(registry, defs), []);
});

// ─── formatReport ────────────────────────────────────────────────────────────

test('formatReport names each problem with what to do about it', () => {
  const text = formatReport({
    exitCode: 2,
    providers: [
      { provider: 'acme', failed: [], new: ['gamma-3'], missing: [{ model: 'acme/beta_2', vendorIds: ['beta-2'] }], changed: ['docs'], sources: [{ key: 'docs', ok: true, ids: 3 }] },
      { provider: 'zeta', failed: [{ source: 'spec', error: 'HTTP 404' }], new: [], missing: [], changed: [], sources: [{ key: 'spec', ok: false, ids: 0, error: 'HTTP 404' }] },
      { provider: 'calm', failed: [], new: [], missing: [], changed: [], sources: [{ key: 'docs', ok: true, ids: 4 }] },
    ],
  });
  assert.match(text, /acme[\s\S]*new upstream[\s\S]*gamma-3/i);
  assert.match(text, /acme\/beta_2[\s\S]*beta-2/);
  assert.match(text, /changed[\s\S]*snapshots\/acme\/docs\.txt/i);
  assert.match(text, /zeta[\s\S]*spec[\s\S]*HTTP 404/);
  assert.doesNotMatch(text, /calm.*(new|missing|FAILED)/i);
});
