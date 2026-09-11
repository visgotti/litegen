#!/usr/bin/env node
// Weekly provider model-drift check — see scripts/model-drift/README.md.
//
//   node scripts/model-drift/check.mjs                 # human report, rewrites snapshots
//   node scripts/model-drift/check.mjs --json          # machine report (the /model-drift skill reads this)
//   node scripts/model-drift/check.mjs --provider runway,openai
//   node scripts/model-drift/check.mjs --no-write      # leave snapshots/ untouched
//   node scripts/model-drift/check.mjs --snapshots /tmp/snap   # compare/write elsewhere
//
// Exit: 0 clean · 1 drift to review · 2 a source/definition needs investigating.
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run, loadDefinitions, formatReport } from './lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const has = (flag) => args.includes(flag);
const valueOf = (flag) => {
  const i = args.indexOf(flag);
  return i >= 0 ? args[i + 1] : undefined;
};

if (has('--help') || has('-h')) {
  process.stdout.write(readmeUsage());
  process.exit(0);
}

try {
  const result = await run({
    modelsDir: process.env.LITEGEN_MODELS_DIR ?? join(here, '..', '..', 'models'),
    definitions: await loadDefinitions(join(here, 'providers')),
    snapshotsDir: valueOf('--snapshots') ?? join(here, 'snapshots'),
    write: !has('--no-write'),
    only: valueOf('--provider')?.split(',').map((s) => s.trim()).filter(Boolean),
  });
  process.stdout.write(has('--json') ? JSON.stringify(result, null, 2) + '\n' : formatReport(result));
  process.exitCode = result.exitCode;
} catch (e) {
  // A broken definition or registry is an investigation case, same as a dead source.
  process.stderr.write(`model-drift: ${e.message}\n`);
  process.exitCode = 2;
}

function readmeUsage() {
  return [
    'usage: node scripts/model-drift/check.mjs [--json] [--provider a,b] [--no-write] [--snapshots dir]',
    '',
    'Compares every vendor\'s published model list against models/*.yaml.',
    'Exit 0 clean · 1 drift to review · 2 a source/definition needs investigating.',
    'Snapshots of each source land in scripts/model-drift/snapshots/ — `git diff` them.',
    '',
  ].join('\n');
}
