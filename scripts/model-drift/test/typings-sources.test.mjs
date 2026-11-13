// Real definitions: a model's typings must come from a snapshot of its own
// request schema, not from a lineup table, an id list or a sibling's schema.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { typingsGaps, validateDefinitions, loadRegistry } from '../lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const def = async (p) => (await import(`../providers/${p}.mjs`)).default;
const registry = loadRegistry(join(here, '..', '..', '..', 'models'));

/** Every source succeeds; `texts` gives some sources' snapshots, the rest are empty. */
const results = (d, texts) => d.sources.map((s) => ({ key: s.key, ok: true, index: Boolean(s.index), ids: ['x'], snapshot: texts[s.key] ?? '' }));

test("vidu's Model Map is the lineup, not a request schema: naming a model there is not its typings", async () => {
  const vidu = await def('vidu');
  const lineup = Object.values(vidu.models).map((id) => `Model | ${id}`).join('\n');
  const gaps = typingsGaps(vidu, results(vidu, { 'model-map': lineup })).map((g) => g.error.split(':')[0]);
  assert.deepEqual(gaps.sort(), Object.keys(vidu.models).sort());
});

test("leonardo/diffusion-xl's typings are the v1 /generations schema, so that source cannot be dropped", async () => {
  const leonardo = await def('leonardo');
  const without = { ...leonardo, sources: leonardo.sources.filter((s) => s.key !== 'generations') };
  const problems = validateDefinitions(new Map([['leonardo', registry.get('leonardo')]]), [without]).map((p) => p.error);
  assert.ok(problems.some((e) => /leonardo\/diffusion-xl.*generations/.test(e)), problems.join('\n') || 'no problem reported');
});

test("pixverse/v5-multi-transition's typings are the multi_transition schema, not pixverse/v5's", async () => {
  const pixverse = await def('pixverse');
  const without = { ...pixverse, sources: pixverse.sources.filter((s) => s.key !== 'multi-transition') };
  const problems = validateDefinitions(new Map([['pixverse', registry.get('pixverse')]]), [without]).map((p) => p.error);
  assert.ok(problems.some((e) => /pixverse\/v5-multi-transition.*multi-transition/.test(e)), problems.join('\n') || 'no problem reported');
});
