// check.mjs's arguments, through the real script. The test copies check.mjs
// next to a stub lib.mjs that records what the script asks run() to do, so no
// case can fetch a vendor page or write a snapshot, whatever the script does
// with its arguments.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const CHECK = join(dirname(fileURLToPath(import.meta.url)), '..', 'check.mjs');

const STUB_LIB = `
import { appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const calls = join(fileURLToPath(new URL('.', import.meta.url)), 'calls.jsonl');
export async function loadDefinitions() { return []; }
export async function run({ definitions, ...opts }) {
  appendFileSync(calls, JSON.stringify({ ...opts, only: opts.only ?? null }) + '\\n');
  return { exitCode: 0, providers: [] };
}
export function formatReport() { return 'stub report\\n'; }
`;

/** Run a copy of check.mjs with `args`; returns the exit status, stderr and every run() call. */
function check(...args) {
  // realpath: node resolves the main module through symlinks (macOS's TMPDIR is under /var -> /private/var).
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'drift-cli-')));
  copyFileSync(CHECK, join(dir, 'check.mjs'));
  writeFileSync(join(dir, 'lib.mjs'), STUB_LIB);
  mkdirSync(join(dir, 'providers'));
  const r = spawnSync(process.execPath, [join(dir, 'check.mjs'), ...args], { cwd: dir, encoding: 'utf8', env: { PATH: process.env.PATH } });
  const log = join(dir, 'calls.jsonl');
  const calls = existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').map((l) => JSON.parse(l)) : [];
  return { status: r.status, stderr: r.stderr, calls, dir };
}

test('check.mjs refuses --provider with no value instead of checking every provider', () => {
  const r = check('--provider');
  assert.deepEqual(r.calls, [], 'run() must not be called');
  assert.equal(r.status, 2);
  assert.match(r.stderr, /--provider/);
});

test('check.mjs reads --provider=<name>, the equals form', () => {
  const r = check('--provider=runway');
  assert.deepEqual(r.calls.map((c) => c.only), [['runway']]);
});

test('check.mjs refuses a provider list that names no provider', () => {
  for (const value of ['', ',', ' , ']) {
    const r = check('--provider', value);
    assert.deepEqual(r.calls, [], `--provider ${JSON.stringify(value)}: run() must not be called`);
    assert.equal(r.status, 2, `--provider ${JSON.stringify(value)}`);
  }
});

test('check.mjs refuses --snapshots with no value or an empty one, instead of writing the default folder', () => {
  for (const args of [['--snapshots'], ['--snapshots', ''], ['--snapshots', '--json']]) {
    const r = check(...args);
    assert.deepEqual(r.calls, [], `${args.join(' ')}: run() must not be called`);
    assert.equal(r.status, 2, args.join(' '));
  }
});

test('check.mjs refuses an unknown flag instead of running with writes on', () => {
  for (const flag of ['--nowrite', '--dry-run', 'runway']) {
    const r = check(flag);
    assert.deepEqual(r.calls, [], `${flag}: run() must not be called`);
    assert.equal(r.status, 2, flag);
    assert.match(r.stderr, new RegExp(flag.replace(/-/g, '\\-')));
  }
});

test('check.mjs passes --no-write, --provider and --snapshots through to run()', () => {
  const r = check('--no-write', '--provider', 'runway,openai', '--snapshots', '/tmp/x', '--json');
  assert.equal(r.status, 0);
  assert.equal(r.calls.length, 1);
  assert.equal(r.calls[0].write, false);
  assert.deepEqual(r.calls[0].only, ['runway', 'openai']);
  assert.equal(r.calls[0].snapshotsDir, '/tmp/x');
});

test('check.mjs with no arguments checks every provider into its own snapshots folder (the weekly run)', () => {
  const r = check();
  assert.equal(r.status, 0);
  assert.equal(r.calls.length, 1);
  assert.equal(r.calls[0].only, null);
  assert.equal(r.calls[0].write, true);
  assert.equal(r.calls[0].snapshotsDir, join(r.dir, 'snapshots'));
});
