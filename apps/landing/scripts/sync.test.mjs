// The three prebuild sync scripts, run for real against a throwaway tree laid
// out like the monorepo (apps/landing/scripts, models/, sdks/openapi.json), so
// the committed generated files are never touched.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPTS = dirname(fileURLToPath(import.meta.url));
const NODE_MODULES = join(SCRIPTS, '..', 'node_modules');
const REPO_MODELS = join(SCRIPTS, '..', '..', '..', 'models');
const OPT_OUT = 'LITEGEN_SYNC_KEEP_COMMITTED';

const GENERATED = [
  'src/config/models.generated.ts',
  'src/components/infra-flow/provider-models.generated.ts',
  'src/components/infra-flow/capabilities.generated.ts',
  'public/openapi.json',
];
const SPEC = JSON.stringify({ openapi: '3.1.0', info: { title: 't', version: '1' }, paths: { '/v1/models': {} } });

/** A monorepo-shaped tree: `models` is 'repo' (the real catalog), 'broken' (one bad yaml) or 'none'; `spec` is sdks/openapi.json's body, or null for none. */
function tree({ models, spec }) {
  const root = mkdtempSync(join(tmpdir(), 'landing-sync-'));
  const landing = join(root, 'apps', 'landing');
  mkdirSync(join(landing, 'scripts'), { recursive: true });
  for (const f of readdirSync(SCRIPTS).filter((f) => f.endsWith('.mjs') && !f.endsWith('.test.mjs'))) copyFileSync(join(SCRIPTS, f), join(landing, 'scripts', f));
  symlinkSync(NODE_MODULES, join(landing, 'node_modules'));
  for (const g of GENERATED) {
    mkdirSync(join(landing, g, '..'), { recursive: true });
    writeFileSync(join(landing, g), `committed ${g}\n`);
  }
  if (models === 'repo') symlinkSync(REPO_MODELS, join(root, 'models'));
  if (models === 'broken') {
    mkdirSync(join(root, 'models'));
    writeFileSync(join(root, 'models', 'acme.yaml'), 'models:\n  - id: acme/x\n    provider: [unclosed\n');
  }
  mkdirSync(join(root, 'sdks'));
  if (spec !== null) writeFileSync(join(root, 'sdks', 'openapi.json'), spec);
  return landing;
}

function sync(landing, script, env = {}) {
  const r = spawnSync(process.execPath, [join('scripts', `${script}.mjs`)], { cwd: landing, encoding: 'utf8', env: { PATH: process.env.PATH, ...env } });
  return { status: r.status, out: r.stdout + r.stderr };
}
const read = (landing, g) => readFileSync(join(landing, g), 'utf8');

test('sync-models and sync-capabilities fail the build when models/*.yaml cannot be read, keeping the committed files', () => {
  for (const script of ['sync-models', 'sync-capabilities']) {
    for (const models of ['broken', 'none']) {
      const landing = tree({ models, spec: SPEC });
      const r = sync(landing, script);
      assert.notEqual(r.status, 0, `${script} with models ${models} exited 0:\n${r.out}`);
      for (const g of GENERATED.slice(0, 3)) assert.equal(read(landing, g), `committed ${g}\n`, `${script} rewrote ${g}`);
    }
  }
});

test('sync-openapi fails the build when sdks/openapi.json is missing', () => {
  const landing = tree({ models: 'repo', spec: null });
  const r = sync(landing, 'sync-openapi');
  assert.notEqual(r.status, 0, r.out);
  assert.equal(read(landing, 'public/openapi.json'), 'committed public/openapi.json\n');
});

test('sync-openapi never copies an empty or non-OpenAPI file over public/openapi.json', () => {
  for (const spec of ['', '<!doctype html>', '{}']) {
    const landing = tree({ models: 'repo', spec });
    const r = sync(landing, 'sync-openapi', { [OPT_OUT]: '1' });
    assert.equal(read(landing, 'public/openapi.json'), 'committed public/openapi.json\n', `${JSON.stringify(spec)} was copied:\n${r.out}`);
    const strict = tree({ models: 'repo', spec });
    assert.notEqual(sync(strict, 'sync-openapi').status, 0, `${JSON.stringify(spec)} passed`);
  }
});

test(`with ${OPT_OUT}=1 a sync that cannot read its source keeps the committed file, warns and exits 0`, () => {
  for (const [script, models, spec, g] of [
    ['sync-models', 'broken', SPEC, 'src/config/models.generated.ts'],
    ['sync-capabilities', 'none', SPEC, 'src/components/infra-flow/capabilities.generated.ts'],
    ['sync-openapi', 'repo', null, 'public/openapi.json'],
  ]) {
    const landing = tree({ models, spec });
    const r = sync(landing, script, { [OPT_OUT]: '1' });
    assert.equal(r.status, 0, `${script}:\n${r.out}`);
    assert.match(r.out, /warning:/);
    assert.equal(read(landing, g), `committed ${g}\n`);
  }
});

test('the sync scripts rewrite the generated files from a readable catalog and spec', () => {
  const landing = tree({ models: 'repo', spec: SPEC });
  for (const script of ['sync-capabilities', 'sync-openapi', 'sync-models']) assert.equal(sync(landing, script).status, 0, script);
  for (const g of GENERATED) assert.notEqual(read(landing, g), `committed ${g}\n`, `${g} was not regenerated`);
  assert.equal(read(landing, 'public/openapi.json'), SPEC);
});
