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

test('loadDefinitions rejects a typings flag that is not a boolean', async () => {
  // typings: 'no' is truthy, so it would count a lineup as typings evidence.
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acme', models: {}, sources: [{ key: 'a', url: 'https://a.test', expect: 'text', extract: /a/g, typings: 'no' }] };`,
  });
  await assert.rejects(loadDefinitions(dir), /acme\.mjs.*"a".*typings/);
});

test('loadDefinitions rejects a typingsFrom that is not a map of model ids to lists of source keys', async () => {
  for (const typingsFrom of [`['schema']`, `'schema'`, `{ 'acme/x': 'schema' }`, `{ 'acme/x': [] }`, `{ 'acme/x': [1] }`]) {
    const dir = defsDir({
      'acme.mjs': `export default { provider: 'acme', models: {}, typingsFrom: ${typingsFrom}, sources: [{ key: 'a', url: 'https://a.test', expect: 'text', extract: /a/g }] };`,
    });
    await assert.rejects(loadDefinitions(dir), /acme\.mjs.*typingsFrom/, `typingsFrom: ${typingsFrom}`);
  }
});

test('loadDefinitions rejects a deprecated reader that is not a function', async () => {
  // A RegExp works for extract, so it is the likely slip here.
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acme', models: {}, sources: [{ key: 'a', url: 'https://a.test', expect: 'text', extract: /a/g, deprecated: /old-\\d/g }] };`,
  });
  await assert.rejects(loadDefinitions(dir), /acme\.mjs.*"a".*deprecated/);
});

test('loadDefinitions accepts a boolean typings flag and a typingsFrom map', async () => {
  const dir = defsDir({
    'acme.mjs': `export default { provider: 'acme', models: {}, typingsFrom: { 'acme/x': ['a'] }, sources: [{ key: 'a', url: 'https://a.test', expect: 'text', extract: /a/g, typings: false }] };`,
  });
  assert.equal((await loadDefinitions(dir)).length, 1);
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
      { provider: 'acme', failed: [], new: ['gamma-3'], missing: [{ model: 'acme/beta_2', vendorIds: ['beta-2', 'beta-2-i2v'], absent: ['beta-2'] }], changed: ['docs'], sources: [{ key: 'docs', ok: true, ids: 3 }] },
      { provider: 'zeta', failed: [{ source: 'spec', error: 'HTTP 404' }], new: [], missing: [], changed: [], sources: [{ key: 'spec', ok: false, ids: 0, error: 'HTTP 404' }] },
      { provider: 'calm', failed: [], new: [], missing: [], changed: [], sources: [{ key: 'docs', ok: true, ids: 4 }] },
    ],
  });
  assert.match(text, /acme[\s\S]*new upstream[\s\S]*gamma-3/i);
  assert.match(text, /acme\/beta_2 \(not found: beta-2\)/);
  assert.match(text, /changed[\s\S]*snapshots\/acme\/docs\.txt/i);
  assert.match(text, /zeta[\s\S]*spec[\s\S]*HTTP 404/);
  assert.doesNotMatch(text, /calm.*(new|missing|FAILED)/i);
});

test('formatReport lists a source with no baseline, so its first snapshot gets a whole-file review', () => {
  const text = formatReport({
    exitCode: 1,
    providers: [
      { provider: 'meshy', failed: [], new: [], missing: [], changed: [], noBaseline: ['openapi'], sources: [{ key: 'openapi', ok: true, ids: 3 }] },
      { provider: 'calm', failed: [], new: [], missing: [], changed: [], noBaseline: [], sources: [{ key: 'docs', ok: true, ids: 4 }] },
    ],
  });
  assert.match(text, /meshy\n[\s\S]*no baseline[\s\S]*openapi/);
  assert.match(text, /clean: calm\n/);
});

test('formatReport lists a carried model the vendor deprecated that models/*.yaml does not record', () => {
  const text = formatReport({
    exitCode: 1,
    providers: [
      { provider: 'fal', failed: [], new: [], missing: [], changed: [], noBaseline: [], deprecated: [{ model: 'fal/auraflow', vendorIds: ['fal-ai/aura-flow'] }], sources: [{ key: 'carried-image', ok: true, ids: 7 }] },
      { provider: 'calm', failed: [], new: [], missing: [], changed: [], noBaseline: [], deprecated: [], sources: [{ key: 'docs', ok: true, ids: 4 }] },
    ],
  });
  assert.match(text, /fal\n[\s\S]*deprecated upstream: fal\/auraflow \(fal-ai\/aura-flow\)/);
  assert.match(text, /clean: calm\n/);
});
