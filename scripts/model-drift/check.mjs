#!/usr/bin/env node
// Weekly provider model-drift check — see scripts/model-drift/README.md.
//
//   node scripts/model-drift/check.mjs                 # human report, rewrites snapshots
//   node scripts/model-drift/check.mjs --json          # machine report (the /model-drift skill reads this)
//   node scripts/model-drift/check.mjs --provider runway,openai
//   node scripts/model-drift/check.mjs --no-write      # leave snapshots/ untouched
//   node scripts/model-drift/check.mjs --snapshots /tmp/snap   # compare/write elsewhere
//
// Exit: 0 clean · 1 drift to review · 2 a source/definition needs investigating,
// or a bad argument. A malformed command line (a flag without its value, an
// unknown flag, a provider list that names no provider, an empty --snapshots)
// stops here, before anything is fetched or written: falling back to "every
// provider, writes on, the repo's snapshots" is what a typo must never do.
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { run, loadDefinitions, formatReport } from './lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));

/** The command line, strictly: anything it does not know is an error, never a default. */
function parseCli(argv) {
  const { values } = parseArgs({
    args: argv,
    strict: true,
    allowPositionals: false,
    options: {
      json: { type: 'boolean' },
      'no-write': { type: 'boolean' },
      provider: { type: 'string', multiple: true },
      snapshots: { type: 'string' },
      help: { type: 'boolean', short: 'h' },
    },
  });
  const only = values.provider?.flatMap((v) => v.split(',')).map((s) => s.trim()).filter(Boolean);
  if (values.provider && only.length === 0) {
    throw new Error('--provider names no provider; give one or more, comma-separated, or leave the flag out to check every provider');
  }
  if (values.snapshots !== undefined && values.snapshots.trim() === '') throw new Error('--snapshots needs a directory');
  return { json: Boolean(values.json), write: !values['no-write'], only, snapshots: values.snapshots, help: Boolean(values.help) };
}

let cli;
try {
  cli = parseCli(process.argv.slice(2));
} catch (e) {
  process.stderr.write(`model-drift: ${e.message}\n\n${usage()}`);
  process.exit(2);
}

if (cli.help) {
  process.stdout.write(usage());
  process.exit(0);
}

try {
  const result = await run({
    modelsDir: process.env.LITEGEN_MODELS_DIR ?? join(here, '..', '..', 'models'),
    definitions: await loadDefinitions(join(here, 'providers')),
    snapshotsDir: cli.snapshots ?? join(here, 'snapshots'),
    write: cli.write,
    only: cli.only,
  });
  process.stdout.write(cli.json ? JSON.stringify(result, null, 2) + '\n' : formatReport(result));
  process.exitCode = result.exitCode;
} catch (e) {
  // A broken definition or registry is an investigation case, same as a dead source.
  process.stderr.write(`model-drift: ${e.message}\n`);
  process.exitCode = 2;
}

function usage() {
  return [
    'usage: node scripts/model-drift/check.mjs [--json] [--provider a,b] [--no-write] [--snapshots dir]',
    '',
    'Compares every vendor\'s published model list against models/*.yaml.',
    'Exit 0 clean · 1 drift to review · 2 a source/definition needs investigating, or a bad argument.',
    'Snapshots of each source land in scripts/model-drift/snapshots/ (or --snapshots dir) — `git diff` them.',
    '',
  ].join('\n');
}
