// pipeline.mjs: the one-command run works on a copy of the baseline, never the
// committed snapshots, and ends with what to do next.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, sep } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { runPipeline, defaultOutDir } from '../pipeline.mjs';

const tmp = (p) => mkdtempSync(join(tmpdir(), p));

/** A catalog of two acme models, a baseline holding acme/docs.txt, and an acme definition reading one page. */
function world({ yamlExtra = '' } = {}) {
  const modelsDir = tmp('drift-pipe-models-');
  writeFileSync(
    join(modelsDir, 'acme.yaml'),
    ['models:', yamlExtra, '  - id: acme/alpha-1', '    provider: acme', '    media_type: image', '  - id: acme/beta_2', '    provider: acme', '    media_type: video', ''].join('\n'),
  );
  const baselineDir = tmp('drift-pipe-base-');
  mkdirSync(join(baselineDir, 'acme'));
  writeFileSync(join(baselineDir, 'acme', 'docs.txt'), 'alpha-1\nbeta-2\n');
  const definitions = [{
    provider: 'acme',
    models: { 'acme/alpha-1': 'alpha-1', 'acme/beta_2': 'beta-2' },
    sources: [{ key: 'docs', url: 'https://acme.test/llms.txt', expect: 'text', extract: /\b(?:alpha|beta|gamma)-\d\b/g }],
    untyped: { 'acme/alpha-1': 'fixture', 'acme/beta_2': 'fixture' },
    acknowledged: [],
  }];
  return { modelsDir, baselineDir, definitions, outDir: join(tmp('drift-pipe-out-'), 'run') };
}

test('the pipeline checks a copy of the baseline, leaves the baseline as it was, and writes every output', async () => {
  const w = world();
  const r = await runPipeline({
    ...w,
    fetchImpl: async () => ({ status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2 gamma-3' }),
  });
  assert.equal(r.exitCode, 1, 'gamma-3 is new upstream');
  assert.equal(readFileSync(join(w.baselineDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1\nbeta-2\n', 'the committed baseline is untouched');
  assert.equal(readFileSync(join(w.outDir, 'snapshots', 'acme', 'docs.txt'), 'utf8'), 'alpha-1\nbeta-2\ngamma-3\n');
  for (const f of ['report.txt', 'drift.json', 'review.md', 'quotes.txt', 'quotes.json', 'summary.txt']) assert.ok(existsSync(join(w.outDir, f)), f);
  assert.match(r.summary, /Review \d+ snapshot sections in .*review\.md/);
  assert.match(r.summary, /acme: new 1/);
  assert.match(r.summary, new RegExp(`cp '${join(w.outDir, 'snapshots', 'acme', 'docs.txt').replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')}' '[^']*acme/docs\\.txt'   # changed`));
  assert.match(readFileSync(join(w.outDir, 'review.md'), 'utf8'), /\+ gamma-3/);
});

test('a broken source puts its repair steps first and exits 2', async () => {
  const w = world();
  const r = await runPipeline({ ...w, fetchImpl: async () => ({ status: 404, contentType: 'text/html', body: '<html>gone</html>' }) });
  assert.equal(r.exitCode, 2);
  assert.match(r.summary, /Next, in this order:\n1\. Repair the broken sources/);
  assert.match(r.summary, /curl -sIL -m 30 'https:\/\/acme\.test\/llms\.txt'/);
  assert.equal(readFileSync(join(w.baselineDir, 'acme', 'docs.txt'), 'utf8'), 'alpha-1\nbeta-2\n', 'a failed source leaves the copy, and so the adoption list, alone');
  assert.doesNotMatch(r.summary, /Adopt each snapshot/);
});

test('the exit code is the worse of the check and the quotes: a dead citation is a 2 even when the check only drifted', async () => {
  const w = world({ yamlExtra: '  # Price: "$0.10 per image at 1024x1024".\n  # @see https://acme.test/pricing.md (2026-11-16)' });
  const r = await runPipeline({
    ...w,
    fetchImpl: async (url) => (url.endsWith('pricing.md') ? { status: 404, body: '' } : { status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2 gamma-3' }),
  });
  assert.equal(r.check.exitCode, 1);
  assert.equal(r.quotes.exitCode, 2);
  assert.equal(r.exitCode, 2);
  assert.match(r.summary, /Fix the dead citations[\s\S]*HTTP 404 · https:\/\/acme\.test\/pricing\.md/);
});

test('a clean run says there is nothing to do', async () => {
  const w = world();
  const r = await runPipeline({ ...w, quotes: false, fetchImpl: async () => ({ status: 200, contentType: 'text/plain', body: 'alpha-1 beta-2' }) });
  assert.equal(r.exitCode, 0);
  assert.match(r.summary, /Nothing to do/);
});

test('a missing baseline is a pipeline error, not a clean run over nothing', async () => {
  const w = world();
  await assert.rejects(runPipeline({ ...w, baselineDir: join(w.baselineDir, 'nope'), fetchImpl: async () => ({ status: 200, body: '' }) }), /no baseline snapshots/);
});

test('the default output folder takes its date and its time from one clock, the local one', () => {
  const tz = process.env.TZ;
  try {
    // Each instant falls on a different calendar day in UTC than where it is read.
    for (const [zone, instant, date, time] of [
      ['America/New_York', '2026-11-17T02:30:05Z', '2026-11-16', '213005'],
      ['Asia/Tokyo', '2026-11-16T15:30:05Z', '2026-11-17', '003005'],
    ]) {
      process.env.TZ = zone;
      assert.deepEqual(defaultOutDir(new Date(instant)).split(sep).slice(-4), ['.superpowers', 'sdd', `${date}-drift`, `run-${time}`], zone);
    }
  } finally {
    if (tz === undefined) delete process.env.TZ;
    else process.env.TZ = tz;
  }
});

// pipeline.mjs's command line, through a copy of the script in a sandbox repo:
// the real review.mjs beside stand-ins for lib.mjs (run() records its call and
// writes one changed snapshot) and quotes.mjs, so no case can fetch a page or
// write outside the sandbox, whatever the script does with its arguments.
const STUB_LIB = `
import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const calls = join(fileURLToPath(new URL('.', import.meta.url)), 'calls.jsonl');
export async function loadDefinitions() { return []; }
export async function run({ snapshotsDir }) {
  appendFileSync(calls, JSON.stringify({ snapshotsDir }) + '\\n');
  mkdirSync(join(snapshotsDir, 'acme'), { recursive: true });
  writeFileSync(join(snapshotsDir, 'acme', 'docs.txt'), '## page\\nrow 1\\nrow 2\\n');
  return { exitCode: 0, providers: [] };
}
export function formatReport() { return 'stub report\\n'; }
export function repairSteps() { return []; }
export function loadRegistry() { return new Map(); }
export function selectProviders() { return []; }
`;
const STUB_QUOTES = `
export async function checkQuotes() { throw new Error('the CLI cases pass --no-quotes'); }
export function formatQuotes() { return ''; }
`;

// A one-model acme catalog and definition, for the sandboxes that run the real
// lib.mjs. Its url is never fetched: those cases are refused before the check.
const ACME_YAML = 'models:\n  - id: acme/alpha-1\n    provider: acme\n    media_type: image\n';
const ACME_DEF = `export default {
  provider: 'acme',
  models: { 'acme/alpha-1': 'alpha-1' },
  sources: [{ key: 'docs', url: 'https://acme.invalid/llms.txt', expect: 'text', extract: /alpha-\\d/g }],
  untyped: { 'acme/alpha-1': 'fixture' },
};
`;

/** Run a sandboxed copy of pipeline.mjs with `args` (plus --out and --no-quotes); returns its exit, output, run() calls and out folder. */
const pipelineCli = (...args) => pipelineCliWith(STUB_LIB, ...args);

/** pipelineCli with `lib` as the sandbox's lib.mjs: a stub's source, or null for the real one. */
function pipelineCliWith(lib, ...args) {
  const scripts = join(dirname(fileURLToPath(import.meta.url)), '..');
  // realpath: node resolves the main module through symlinks (macOS's TMPDIR is under /var -> /private/var).
  const root = realpathSync(tmp('drift-pipe-cli-'));
  const dir = join(root, 'scripts', 'model-drift');
  mkdirSync(join(dir, 'snapshots', 'acme'), { recursive: true });
  mkdirSync(join(dir, 'providers'));
  mkdirSync(join(root, 'models'));
  copyFileSync(join(scripts, 'pipeline.mjs'), join(dir, 'pipeline.mjs'));
  copyFileSync(join(scripts, 'review.mjs'), join(dir, 'review.mjs'));
  if (lib === null) copyFileSync(join(scripts, 'lib.mjs'), join(dir, 'lib.mjs'));
  else writeFileSync(join(dir, 'lib.mjs'), lib);
  writeFileSync(join(dir, 'quotes.mjs'), STUB_QUOTES);
  writeFileSync(join(dir, 'providers', 'acme.mjs'), ACME_DEF);
  writeFileSync(join(root, 'models', 'acme.yaml'), ACME_YAML);
  writeFileSync(join(dir, 'snapshots', 'acme', 'docs.txt'), '## page\nrow 0\n');
  const out = join(root, 'out');
  const r = spawnSync(process.execPath, [join(dir, 'pipeline.mjs'), '--out', out, '--no-quotes', ...args], { cwd: root, encoding: 'utf8', env: { PATH: process.env.PATH } });
  const log = join(dir, 'calls.jsonl');
  return { status: r.status, stdout: r.stdout, stderr: r.stderr, calls: existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n') : [], out };
}

test('pipeline.mjs refuses a --max-lines that is not a whole number with a usage error and exit 2, before it runs or writes anything', () => {
  for (const args of [['--max-lines', 'abc'], ['--max-lines', ''], ['--max-lines=-1'], ['--max-lines', '2.5']]) {
    const r = pipelineCli(...args);
    assert.equal(r.status, 2, `${args.join(' ')}: ${r.stdout}${r.stderr}`);
    assert.match(r.stderr, /--max-lines must be a whole number[\s\S]*usage: /, args.join(' '));
    assert.deepEqual(r.calls, [], `${args.join(' ')}: the check must not run`);
    assert.equal(existsSync(r.out), false, `${args.join(' ')}: nothing is written`);
  }
  const ok = pipelineCli('--max-lines', '1');
  assert.equal(ok.status, 0, ok.stderr);
  assert.equal(ok.calls.length, 1);
  assert.match(readFileSync(join(ok.out, 'review.md'), 'utf8'), /^ {2}- row 0\n {2}… 2 more lines/m);
});

test('pipeline.mjs refuses a provider list that names no provider, or one the catalog does not know, with a usage error and exit 2, before it writes anything', () => {
  for (const [args, message] of [
    [['--provider', ','], /names no provider/],
    [['--provider', ' , '], /names no provider/],
    [['--provider', 'nosuch'], /unknown provider\(s\): nosuch/],
    [['--provider', 'acme,nosuch'], /unknown provider\(s\): nosuch/],
  ]) {
    const r = pipelineCliWith(null, ...args);
    assert.equal(r.status, 2, `${args.join(' ')}: ${r.stderr}`);
    assert.match(r.stderr, message, args.join(' '));
    assert.match(r.stderr, /usage: node scripts\/model-drift\/pipeline\.mjs/, args.join(' '));
    assert.doesNotMatch(r.stderr, /PIPELINE BROKEN/, `${args.join(' ')}: exit 3 says the scripts are broken`);
    assert.equal(existsSync(r.out), false, `${args.join(' ')}: nothing is written`);
  }
});
