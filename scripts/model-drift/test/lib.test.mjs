// Unit tests for the model-drift core. Run: node --test scripts/model-drift/test/
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadRegistry, evaluateSource, classify, typingsGaps, run } from '../lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const FIXTURE_MODELS = join(here, 'fixtures', 'models');

// ─── loadRegistry ────────────────────────────────────────────────────────────

test('loadRegistry groups model ids by provider and skips mock', () => {
  const reg = loadRegistry(FIXTURE_MODELS);
  assert.deepEqual([...reg.keys()], ['acme']);
  assert.deepEqual(reg.get('acme'), ['acme/alpha-1', 'acme/beta_2']);
});

test('loadRegistry fails loudly when a model block has no id line it can parse', () => {
  const dir = mkdtempSync(join(tmpdir(), 'drift-reg-'));
  // Three media_type lines but only two `- id:` lines → the line parser would
  // silently drop a model; it must refuse instead.
  writeFileSync(join(dir, 'acme.yaml'), [
    'models:',
    '  - id: acme/a', '    provider: acme', '    media_type: image',
    '  - { id: acme/b, provider: acme }', '    media_type: image',
    '  - id: acme/c', '    provider: acme', '    media_type: video',
  ].join('\n'));
  assert.throws(() => loadRegistry(dir), /acme\.yaml.*2 ids.*3 models/);
});

// ─── evaluateSource ──────────────────────────────────────────────────────────

const html = '<!DOCTYPE html><html><head><title>Docs</title></head><body>app shell</body></html>';

test('evaluateSource reports the HTTP status for an error response', () => {
  const r = evaluateSource({ key: 's', expect: 'json', extract: /x/g }, { status: 404, contentType: 'text/html', body: html });
  assert.equal(r.ok, false);
  assert.match(r.error, /HTTP 404/);
});

test('evaluateSource treats an HTML page served where JSON was expected as a soft 404', () => {
  const r = evaluateSource({ key: 's', expect: 'json', extract: () => ['x'] }, { status: 200, contentType: 'text/html', body: html });
  assert.equal(r.ok, false);
  assert.match(r.error, /expected json.*HTML/i);
});

test('evaluateSource treats an HTML shell served where markdown was expected as a soft 404', () => {
  const r = evaluateSource({ key: 's', expect: 'text', extract: /gen\d/g }, { status: 200, contentType: 'text/html', body: html });
  assert.equal(r.ok, false);
  assert.match(r.error, /expected text.*HTML/i);
});

test('evaluateSource extracts ids with a regex, lower-cased, trimmed of trailing separators, deduped and sorted', () => {
  const body = 'Use Sora-2-Pro or sora-2. Also sora-2- (hyphen artefact) and GPT-IMAGE-1.';
  const r = evaluateSource(
    { key: 's', expect: 'text', extract: /sora-2[a-z0-9-]*|gpt-image-[0-9a-z.-]+/gi },
    { status: 200, contentType: 'text/plain', body },
  );
  assert.equal(r.ok, true);
  assert.deepEqual(r.ids, ['gpt-image-1', 'sora-2', 'sora-2-pro']);
});

test('evaluateSource runs a function extractor against parsed JSON', () => {
  const body = JSON.stringify({ models: [{ id: 'flux-a' }, { id: 'flux-b' }] });
  const r = evaluateSource(
    { key: 's', expect: 'json', extract: (doc) => doc.models.map((m) => m.id) },
    { status: 200, contentType: 'application/json', body },
  );
  assert.deepEqual(r.ids, ['flux-a', 'flux-b']);
});

test('evaluateSource fails when the extractor finds no ids at all', () => {
  const r = evaluateSource({ key: 's', expect: 'text', extract: /gen\d/g }, { status: 200, contentType: 'text/plain', body: 'nothing here' });
  assert.equal(r.ok, false);
  assert.match(r.error, /no model ids/i);
});

test('evaluateSource reports an extractor that throws instead of crashing the run', () => {
  const r = evaluateSource(
    { key: 's', expect: 'json', extract: (doc) => doc.missing.map((m) => m.id) },
    { status: 200, contentType: 'application/json', body: '{}' },
  );
  assert.equal(r.ok, false);
  assert.match(r.error, /extract/i);
});

test('evaluateSource snapshots the typings excerpt when the source defines one, else the id list', () => {
  const src = { key: 's', expect: 'json', extract: (d) => d.ids, snapshot: (d) => `sizes=${d.sizes.join(',')}` };
  const r = evaluateSource(src, { status: 200, contentType: 'application/json', body: '{"ids":["b","a"],"sizes":["1024x1024"]}' });
  assert.equal(r.snapshot, 'sizes=1024x1024\n');
  const bare = evaluateSource({ key: 's', expect: 'json', extract: (d) => d.ids }, { status: 200, contentType: 'application/json', body: '{"ids":["b","a"]}' });
  assert.equal(bare.snapshot, 'a\nb\n');
});

test('evaluateSource hands the snapshot our vendor ids so an excerpt can be scoped to carried models', () => {
  const src = { key: 's', expect: 'json', extract: (d) => d.ids, snapshot: (d, _body, ctx) => ctx.ours.join('+') };
  const r = evaluateSource(src, { status: 200, contentType: 'application/json', body: '{"ids":["a"]}' }, { ours: ['a', 'b'] });
  assert.equal(r.snapshot, 'a+b\n');
});

// ─── classify ────────────────────────────────────────────────────────────────

const def = {
  provider: 'acme',
  models: { 'acme/alpha-1': 'alpha-1', 'acme/beta_2': ['beta-2', 'beta-2-i2v'] },
  acknowledged: [
    { id: 'legacy-0', reason: 'shut down upstream, docs still list it' },
    { pattern: /^alpha-1-\d{8}$/, reason: 'dated snapshot aliases' },
  ],
};
const ok = (key, ids) => ({ key, ok: true, ids });

test('classify lists upstream ids we neither carry nor acknowledged as new', () => {
  const r = classify(def, [ok('a', ['alpha-1', 'beta-2', 'beta-2-i2v', 'gamma-3', 'legacy-0', 'alpha-1-20260101'])]);
  assert.deepEqual(r.new, ['gamma-3']);
  assert.deepEqual(r.missing, []);
  assert.deepEqual(r.failed, []);
});

test('classify matches our vendor ids case-insensitively', () => {
  const r = classify({ ...def, models: { 'acme/alpha-1': 'Alpha-1' } }, [ok('a', ['alpha-1'])]);
  assert.deepEqual(r.new, []);
  assert.deepEqual(r.missing, []);
});

test('classify flags a carried model none of whose vendor ids appear upstream as missing', () => {
  const r = classify(def, [ok('a', ['alpha-1'])]);
  assert.deepEqual(r.missing, [{ model: 'acme/beta_2', vendorIds: ['beta-2', 'beta-2-i2v'], absent: ['beta-2', 'beta-2-i2v'] }]);
});

test('classify flags a model one of whose endpoints is gone, even while the others remain', () => {
  // The adapter sends every mapped id (t2v and i2v here), so i2v vanishing breaks it.
  const r = classify(def, [ok('a', ['alpha-1', 'beta-2'])]);
  assert.deepEqual(r.missing, [{ model: 'acme/beta_2', vendorIds: ['beta-2', 'beta-2-i2v'], absent: ['beta-2-i2v'] }]);
});

test('classify counts a model present when every one of its vendor ids appears', () => {
  const r = classify(def, [ok('a', ['alpha-1', 'beta-2', 'beta-2-i2v'])]);
  assert.deepEqual(r.missing, []);
});

test('classify withholds missing when any source failed, since the model may live in that source', () => {
  const r = classify(def, [ok('a', ['alpha-1']), { key: 'b', ok: false, error: 'HTTP 404' }]);
  assert.deepEqual(r.missing, []);
  assert.deepEqual(r.failed, [{ source: 'b', error: 'HTTP 404' }]);
});

test('classify fails the provider when sources return ids but none of ours, suppressing new/missing noise', () => {
  const r = classify(def, [ok('a', ['totally', 'unrelated', 'ids'])]);
  assert.deepEqual(r.new, []);
  assert.deepEqual(r.missing, []);
  assert.equal(r.failed.length, 1);
  assert.match(r.failed[0].error, /none of our 2 models/i);
});

// ─── typingsGaps ──────────────────────────────────────────────────────────────

const typedDef = (extra = {}) => ({
  provider: 'acme',
  models: { 'acme/alpha-1': 'alpha-1', 'acme/beta_2': ['beta-2', 'beta-2-i2v'] },
  sources: [
    { key: 'list', extract: /x/ },
    { key: 'schema', extract: /x/, snapshot: () => '' },
    { key: 'pages', index: true, extract: /x/, snapshot: () => '' },
    { key: 'fields', index: true, typings: true, extract: /x/, snapshot: () => '' },
  ],
  ...extra,
});
const snap = (key, snapshot, more = {}) => ({ key, ok: true, ids: ['x'], snapshot, ...more });

test('typingsGaps passes when every vendor id appears in a typings snapshot', () => {
  const gaps = typingsGaps(typedDef(), [snap('list', 'zzz'), snap('schema', 'model: [alpha-1, beta-2]\nPOST /v1/beta-2-i2v')]);
  assert.deepEqual(gaps, []);
});

test('typingsGaps flags a carried vendor id that no typings snapshot mentions', () => {
  const gaps = typingsGaps(typedDef(), [snap('schema', 'model: [alpha-1, beta-2]')]);
  assert.equal(gaps.length, 1);
  assert.match(gaps[0].error, /acme\/beta_2: beta-2-i2v appears in no typings snapshot/);
});

test('typingsGaps does not count id-list or index snapshots as typings evidence', () => {
  const gaps = typingsGaps(typedDef(), [
    snap('list', 'alpha-1\nbeta-2\nbeta-2-i2v'),
    snap('schema', ''),
    snap('pages', 'alpha-1 beta-2 beta-2-i2v', { index: true }),
  ]);
  assert.deepEqual(gaps.map((g) => g.error.split(':')[0]), ['acme/alpha-1', 'acme/beta_2']);
});

test('typingsGaps matches whole ids only, so a longer sibling id is not coverage', () => {
  const def = { ...typedDef(), models: { 'acme/alpha-1': 'alpha-1' } };
  assert.equal(typingsGaps(def, [snap('schema', '"alpha-1-pro" alpha-12 alpha-1_turbo v.alpha-1')]).length, 1);
  assert.deepEqual(typingsGaps(def, [snap('schema', 'enum: ["ALPHA-1", "alpha-1-pro"]. done')]), []);
  assert.deepEqual(typingsGaps(def, [snap('schema', 'POST /v1/alpha-1 (application/json)')]), []);
  assert.deepEqual(typingsGaps(def, [snap('schema', 'POST /v1/alpha-1/generate (multipart/form-data)')]), []);
});

test('typingsGaps counts an index source that declares its snapshot typings', () => {
  assert.deepEqual(typingsGaps(typedDef(), [snap('fields', '// action alpha-1\n// action beta-2\n// action beta-2-i2v', { index: true })]), []);
});

test('typingsGaps skips a model listed in untyped', () => {
  const def = typedDef({ untyped: { 'acme/beta_2': 'vendor publishes no request schema' } });
  assert.deepEqual(typingsGaps(def, [snap('schema', 'alpha-1')]), []);
});

test('typingsGaps leaves out a model already reported missing upstream', () => {
  assert.deepEqual(typingsGaps(typedDef(), [snap('schema', 'alpha-1')], ['acme/beta_2']), []);
});

test('typingsGaps is withheld when a typings source failed, since the id may live in it', () => {
  assert.deepEqual(typingsGaps(typedDef(), [{ key: 'schema', ok: false, error: 'HTTP 404' }]), []);
});

test('classify leaves index sources (page or endpoint lists) out of the model-id comparison', () => {
  const r = classify(def, [ok('a', ['alpha-1', 'beta-2', 'beta-2-i2v']), { key: 'pages', ok: true, index: true, ids: ['/docs/new-video-api'] }]);
  assert.deepEqual(r.new, []);
  assert.deepEqual(r.missing, []);
  assert.deepEqual(r.failed, []);
});

test('evaluateSource marks results from an index source', () => {
  const r = evaluateSource({ key: 'pages', index: true, expect: 'text', extract: /\/docs\/[a-z-]+/g }, { status: 200, contentType: 'text/plain', body: '/docs/video' });
  assert.equal(r.index, true);
});

// ─── run (end to end with a fake network) ────────────────────────────────────

function fakeFetch(routes) {
  return async (url) => {
    const r = routes[url];
    if (!r) throw new Error(`unexpected fetch ${url}`);
    if (r instanceof Error) throw r;
    return r;
  };
}

function acmeDef(url) {
  return {
    provider: 'acme',
    models: { 'acme/alpha-1': 'alpha-1', 'acme/beta_2': 'beta-2' },
    sources: [{ key: 'docs', url, expect: 'text', extract: /\b(?:alpha|beta|gamma)-\d\b/g }],
    // This fixture's one source is an id list; typings coverage has its own tests.
    untyped: { 'acme/alpha-1': 'fixture: id list only', 'acme/beta_2': 'fixture: id list only' },
    acknowledged: [],
  };
}

test('run exits 0 and writes the snapshot when upstream matches what we carry', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 and beta-2' } }),
  });
  assert.equal(res.exitCode, 0);
  assert.equal(readFileSync(join(snapDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1\nbeta-2\n');
});

test('run exits 1 and reports the new model when upstream adds one', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2 gamma-3' } }),
  });
  assert.equal(res.exitCode, 1);
  assert.deepEqual(res.providers[0].new, ['gamma-3']);
});

test('run exits 2 when a source is unreachable, even if other providers drifted', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const good = 'https://acme.test/llms.txt';
  const dead = 'https://other.test/models';
  const other = {
    provider: 'other', models: {}, acknowledged: [],
    sources: [{ key: 'list', url: dead, expect: 'json', extract: (d) => d }],
  };
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(good), other],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({
      [good]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2 gamma-3' },
      [dead]: new Error('getaddrinfo ENOTFOUND other.test'),
    }),
  });
  assert.equal(res.exitCode, 2);
  const o = res.providers.find((p) => p.provider === 'other');
  assert.match(o.failed[0].error, /ENOTFOUND/);
});

test('run marks a source changed when its snapshot differs from the committed one, then rewrites it', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  mkdirSync(join(snapDir, 'acme'), { recursive: true });
  writeFileSync(join(snapDir, 'acme', 'docs.txt'), 'alpha-1\n');
  const url = 'https://acme.test/llms.txt';
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.deepEqual(res.providers[0].changed, ['docs']);
  assert.equal(res.exitCode, 1);
  assert.equal(readFileSync(join(snapDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1\nbeta-2\n');
});

test('run with write:false leaves snapshots untouched', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  mkdirSync(join(snapDir, 'acme'), { recursive: true });
  writeFileSync(join(snapDir, 'acme', 'docs.txt'), 'alpha-1\n');
  const url = 'https://acme.test/llms.txt';
  await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    write: false,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.equal(readFileSync(join(snapDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1\n');
});

test('run flags a registry model the provider map does not cover, and exits 2', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const partial = { ...acmeDef(url), models: { 'acme/alpha-1': 'alpha-1' } };
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [partial],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.equal(res.exitCode, 2);
  assert.ok(res.providers[0].failed.some((f) => /acme\/beta_2.*not mapped/.test(f.error)));
});

test('run flags a mapped model that no longer exists in the registry', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const stale = { ...acmeDef(url), models: { ...acmeDef(url).models, 'acme/retired': 'retired-0' } };
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [stale],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.equal(res.exitCode, 2);
  assert.ok(res.providers[0].failed.some((f) => /acme\/retired.*not in models/.test(f.error)));
});

test('run flags a registry provider that has no drift definition at all', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const res = await run({ modelsDir: FIXTURE_MODELS, definitions: [], snapshotsDir: snapDir, fetchImpl: fakeFetch({}) });
  assert.equal(res.exitCode, 2);
  assert.equal(res.providers[0].provider, 'acme');
  assert.match(res.providers[0].failed[0].error, /no drift definition/);
});

test('run rejects an unknown provider filter instead of reporting a clean run over nothing', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  await assert.rejects(
    run({ modelsDir: FIXTURE_MODELS, definitions: [acmeDef('https://acme.test/x')], snapshotsDir: snapDir, only: ['acmee'], fetchImpl: fakeFetch({}) }),
    /unknown provider.*acmee/,
  );
});

test('run passes each provider its carried vendor ids for snapshot scoping', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const def = acmeDef(url);
  def.sources[0].snapshot = (_p, _b, ctx) => ctx.ours.join(',');
  await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [def],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.equal(readFileSync(join(snapDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1,beta-2\n');
});

test('run deletes the snapshot of a source that no longer exists in the definition', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  mkdirSync(join(snapDir, 'acme'), { recursive: true });
  writeFileSync(join(snapDir, 'acme', 'old-page.txt'), 'alpha-1\n');
  const url = 'https://acme.test/llms.txt';
  await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.deepEqual(readdirSync(join(snapDir, 'acme')), ['docs.txt']);
});

test('run re-fetches once when a 200 response fails extraction, so a CDN serving a stray shell is not a false alarm', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const answers = [
    { status: 200, contentType: 'text/html', body: html },
    { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' },
  ];
  let calls = 0;
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    fetchImpl: async () => answers[calls++],
  });
  assert.equal(calls, 2);
  assert.equal(res.exitCode, 0);
});

test('run does not re-fetch an HTTP error (the fetcher already retried transient ones)', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  let calls = 0;
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef('https://acme.test/llms.txt')],
    snapshotsDir: snapDir,
    fetchImpl: async () => { calls++; return { status: 404, contentType: 'text/html', body: html }; },
  });
  assert.equal(calls, 1);
  assert.equal(res.exitCode, 2);
});

test('run resolves a source url through its discover page, for content-hashed assets', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const def = acmeDef('unused');
  def.sources[0] = { ...def.sources[0], url: undefined, discover: { url: 'https://acme.test/docs', pattern: /\/assets\/index-[\w-]+\.js/ } };
  const seen = [];
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [def],
    snapshotsDir: snapDir,
    fetchImpl: async (url) => {
      seen.push(url);
      if (url === 'https://acme.test/docs') return { status: 200, contentType: 'text/html', body: '<script src="/assets/index-Ab12.js"></script>' };
      if (url === 'https://acme.test/assets/index-Ab12.js') return { status: 200, contentType: 'text/javascript', body: 'alpha-1 beta-2' };
      throw new Error(`unexpected ${url}`);
    },
  });
  assert.deepEqual(seen, ['https://acme.test/docs', 'https://acme.test/assets/index-Ab12.js']);
  assert.equal(res.exitCode, 0);
  assert.equal(res.providers[0].sources[0].url, 'https://acme.test/assets/index-Ab12.js');
});

test('run fails a source whose discover page no longer contains the asset pattern', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const def = acmeDef('unused');
  def.sources[0] = { ...def.sources[0], url: undefined, discover: { url: 'https://acme.test/docs', pattern: /\/assets\/index-[\w-]+\.js/ } };
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [def],
    snapshotsDir: snapDir,
    fetchImpl: async () => ({ status: 200, contentType: 'text/html', body: '<html>rebuilt docs</html>' }),
  });
  assert.equal(res.exitCode, 2);
  assert.match(res.providers[0].failed[0].error, /discover.*acme\.test\/docs.*no match/);
});

test('run does not overwrite the committed snapshot of a failed source', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  mkdirSync(join(snapDir, 'acme'), { recursive: true });
  writeFileSync(join(snapDir, 'acme', 'docs.txt'), 'alpha-1\nbeta-2\n');
  const url = 'https://acme.test/llms.txt';
  await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [acmeDef(url)],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 404, contentType: 'text/html', body: html } }),
  });
  assert.equal(readFileSync(join(snapDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1\nbeta-2\n');
});

test('run exits 2 when a carried model is in no typings snapshot', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/spec';
  const def = { ...acmeDef(url), untyped: undefined };
  def.sources = [{ ...def.sources[0], snapshot: () => 'model: [alpha-1]' }];
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [def],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.equal(res.exitCode, 2);
  assert.deepEqual(res.providers[0].failed.map((f) => f.source), ['(typings)']);
  assert.match(res.providers[0].failed[0].error, /acme\/beta_2: beta-2/);
});

test('run flags an untyped entry that names no mapped model, or gives no reason', async () => {
  const snapDir = mkdtempSync(join(tmpdir(), 'drift-snap-'));
  const url = 'https://acme.test/llms.txt';
  const def = { ...acmeDef(url), untyped: { 'acme/alpha-1': '', 'acme/beta_2': 'id list only', 'acme/gone': 'retired' } };
  const res = await run({
    modelsDir: FIXTURE_MODELS,
    definitions: [def],
    snapshotsDir: snapDir,
    fetchImpl: fakeFetch({ [url]: { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' } }),
  });
  assert.equal(res.exitCode, 2);
  const errors = res.providers[0].failed.map((f) => f.error).join('\n');
  assert.match(errors, /untyped acme\/alpha-1 .* needs a reason/);
  assert.match(errors, /untyped acme\/gone .* is not a mapped model/);
});
