#!/usr/bin/env node
// The whole weekly drift pipeline in one command, safe by default:
//
//   node scripts/model-drift/pipeline.mjs [--provider a,b] [--out dir] [--no-quotes] [--max-lines 60]
//
// 1. copies the committed snapshots (the reviewed baseline) into <out>/snapshots
//    and runs the check there, so scripts/model-drift/snapshots is never
//    written (other sessions may have edits in it);
// 2. writes the check's report (<out>/report.txt, with a repair recipe for
//    every FAILED source) and JSON (<out>/drift.json);
// 3. writes the review packet (<out>/review.md, review.mjs): only the
//    snapshot sections that changed, by JSON path, and the never-reviewed
//    sections to read whole;
// 4. checks every quoted vendor string in models/*.yaml against the pages it
//    cites (<out>/quotes.txt / quotes.json, quotes.mjs): prices and rules a
//    vendor changed, and citations that died;
// 5. prints what to do next, in order: repair, review, decide, fix quotes,
//    then adopt reviewed snapshots file by file (the exact cp/rm commands).
//
// <out> defaults to .superpowers/sdd/<date>-drift/run-<time>/, both local
// (git-ignored, and unlike /tmp it survives a restart).
//
// Exit: 0 clean · 1 drift or changed quotes to review · 2 a source, definition
// or cited page is broken (repair first), or the command line is (a usage
// message; nothing runs) · 3 the pipeline itself crashed (a bug in
// scripts/model-drift: fix it, then run the node tests).
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, dirname, resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { execFileSync } from 'node:child_process';
import { run, loadDefinitions, loadRegistry, selectProviders, formatReport, repairSteps } from './lib.mjs';
import { review, formatReview, parseMaxLines } from './review.mjs';
import { checkQuotes, formatQuotes } from './quotes.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '..', '..');

/** `git status --short` of a folder, or null outside a git checkout. */
function gitStatus(dir) {
  try {
    return execFileSync('git', ['status', '--short', '--', dir], { cwd: REPO, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
  } catch {
    return null;
  }
}

const txt = (dir) => (existsSync(dir) ? readdirSync(dir).filter((f) => f.endsWith('.txt')) : []);
const dirs = (dir) => (existsSync(dir) ? readdirSync(dir).filter((f) => statSync(join(dir, f)).isDirectory()) : []);

/** The adoption commands for every snapshot file the run changed, added or dropped (relative to the repo). */
export function adoptCommands(baselineDir, freshDir, only) {
  const rel = (p) => relative(REPO, p) || '.';
  const out = [];
  for (const p of [...new Set([...dirs(baselineDir), ...dirs(freshDir)])].filter((x) => !only || only.includes(x)).sort()) {
    const before = txt(join(baselineDir, p));
    const after = txt(join(freshDir, p));
    for (const f of [...new Set([...before, ...after])].sort()) {
      const b = join(baselineDir, p, f);
      const a = join(freshDir, p, f);
      if (!after.includes(f)) out.push(`rm '${rel(b)}'   # source removed from the definition`);
      else if (!before.includes(f)) out.push(`mkdir -p '${rel(dirname(b))}' && cp '${a}' '${rel(b)}'   # new: read it whole first`);
      else if (readFileSync(a, 'utf8') !== readFileSync(b, 'utf8')) out.push(`cp '${a}' '${rel(b)}'   # changed: review its sections first`);
    }
  }
  return out;
}

/**
 * Run the pipeline. Everything it writes goes under `outDir`.
 * @returns {{ exitCode, outDir, check, quotes, summary }}
 */
export async function runPipeline({ modelsDir, definitions, baselineDir, outDir, only, fetchImpl, quotes = true, knownFile, maxLines = 60 }) {
  // A provider filter that names none, or one the catalog does not know, throws
  // here (`usage`), before anything is written: the CLI answers it with exit 2.
  selectProviders(loadRegistry(modelsDir), definitions, only);
  mkdirSync(outDir, { recursive: true });
  const freshDir = join(outDir, 'snapshots');
  if (!existsSync(baselineDir)) throw new Error(`no baseline snapshots at ${baselineDir}`);
  cpSync(baselineDir, freshDir, { recursive: true });

  const check = await run({ modelsDir, definitions, snapshotsDir: freshDir, write: true, only, ...(fetchImpl ? { fetchImpl } : {}) });
  writeFileSync(join(outDir, 'drift.json'), JSON.stringify(check, null, 2) + '\n');
  writeFileSync(join(outDir, 'report.txt'), formatReport(check));

  const packet = formatReview(review({ baselineDir, freshDir, only }), { maxLines, freshDir });
  writeFileSync(join(outDir, 'review.md'), packet);

  let q = null;
  if (quotes) {
    q = await checkQuotes({ modelsDir, only, knownFile, ...(fetchImpl ? { fetchImpl } : {}) });
    writeFileSync(join(outDir, 'quotes.json'), JSON.stringify(q, null, 2) + '\n');
    writeFileSync(join(outDir, 'quotes.txt'), formatQuotes(q));
  }

  const exitCode = Math.max(check.exitCode, q?.exitCode ?? 0);
  const summary = summarize({ check, q, outDir, baselineDir, freshDir, only, packet });
  writeFileSync(join(outDir, 'summary.txt'), summary);
  return { exitCode, outDir, check, quotes: q, summary };
}

function summarize({ check, q, outDir, baselineDir, freshDir, only, packet }) {
  const ps = check.providers;
  const count = (k) => ps.reduce((n, p) => n + (p[k]?.length ?? 0), 0);
  const lines = [
    `Model-drift pipeline · ${ps.length} providers · check exit ${check.exitCode}${q ? ` · quotes exit ${q.exitCode}` : ''}`,
    `outputs: ${outDir}/{report.txt, drift.json, review.md${q ? ', quotes.txt, quotes.json' : ''}, snapshots/}`,
    `FAILED ${count('failed')} · new ${count('new')} · missing ${count('missing')} · deprecated ${count('deprecated')} · changed ${count('changed')} · no baseline ${count('noBaseline')}` +
      (q ? ` · quotes not on their pages ${q.missing.length} · dead citations ${q.failedUrls.length}` : ''),
    '',
    'Next, in this order:',
  ];
  let step = 1;
  const repair = repairSteps(ps);
  if (repair.length) {
    lines.push(`${step++}. Repair the broken sources (then rerun this pipeline):`, ...repair.map((l) => `   ${l}`));
  }
  if (q?.failedUrls.length) {
    lines.push(`${step++}. Fix the dead citations (\`@see\` in models/*.yaml; see ${outDir}/quotes.txt):`);
    for (const f of q.failedUrls.slice(0, 15)) lines.push(`   ${f.error} · ${f.url}`);
    if (q.failedUrls.length > 15) lines.push(`   … ${q.failedUrls.length - 15} more in quotes.txt`);
  }
  const reviewItems = (packet.match(/^## /gm) ?? []).length;
  if (count('changed') || count('noBaseline')) {
    lines.push(`${step++}. Review ${reviewItems} snapshot sections in ${outDir}/review.md against models/<provider>.yaml (typings recipe in .claude/skills/model-drift/SKILL.md).`);
  }
  const decide = ps.filter((p) => p.new.length || p.missing.length || p.deprecated?.length);
  if (decide.length) {
    lines.push(`${step++}. Decide new / missing / deprecated models (${outDir}/report.txt):`);
    for (const p of decide) {
      const bits = [p.new.length && `new ${p.new.length}`, p.missing.length && `missing ${p.missing.map((m) => m.model).join(', ')}`, p.deprecated?.length && `deprecated ${p.deprecated.map((d) => d.model).join(', ')}`].filter(Boolean);
      lines.push(`   ${p.provider}: ${bits.join(' · ')}`);
    }
  }
  if (q?.missing.length) {
    lines.push(`${step++}. Re-read the pages of ${q.missing.length} quotes that are no longer on them, and fix the row and the quote (${outDir}/quotes.txt).`);
  }
  const adopt = adoptCommands(baselineDir, freshDir, only);
  if (adopt.length) {
    lines.push(`${step++}. Adopt each snapshot only after reviewing it, file by file (next week's baseline):`, ...adopt.map((c) => `   ${c}`));
  }
  if (step === 1) lines.push('Nothing to do: every source, snapshot and quote matches.');
  return lines.join('\n') + '\n';
}

/**
 * Where a run writes when `--out` is not given: .superpowers/sdd/<date>-drift/run-<time>/,
 * both read from the local clock (a UTC date beside a local time files an
 * evening run under tomorrow's folder).
 */
export function defaultOutDir(now = new Date()) {
  const two = (n) => String(n).padStart(2, '0');
  const date = `${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}`;
  const time = `${two(now.getHours())}${two(now.getMinutes())}${two(now.getSeconds())}`;
  return join(REPO, '.superpowers', 'sdd', `${date}-drift`, `run-${time}`);
}

// ─── CLI ─────────────────────────────────────────────────────────────────────

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const USAGE = 'usage: node scripts/model-drift/pipeline.mjs [--provider a,b] [--out dir] [--no-quotes] [--max-lines 60]\n';
  let values;
  let only;
  let maxLines;
  try {
    ({ values } = parseArgs({
      args: process.argv.slice(2),
      strict: true,
      allowPositionals: false,
      options: {
        provider: { type: 'string', multiple: true },
        out: { type: 'string' },
        'no-quotes': { type: 'boolean' },
        'max-lines': { type: 'string' },
        help: { type: 'boolean', short: 'h' },
      },
    }));
    // Refused here, before anything runs: exit 2 is a bad command line, and a
    // throw inside the run would read as exit 3, a bug in these scripts.
    only = values.provider?.flatMap((v) => v.split(',')).map((s) => s.trim()).filter(Boolean);
    if (values.provider && only.length === 0) {
      throw new Error('--provider names no provider; give one or more, comma-separated, or leave the flag out to check every provider');
    }
    maxLines = parseMaxLines(values['max-lines']);
  } catch (e) {
    process.stderr.write(`pipeline: ${e.message}\n${USAGE}`);
    process.exit(2);
  }
  if (values.help) {
    process.stdout.write(USAGE);
    process.exit(0);
  }
  const outDir = resolve(values.out ?? defaultOutDir());
  const baselineDir = join(here, 'snapshots');
  const before = gitStatus(baselineDir);
  try {
    const r = await runPipeline({
      modelsDir: process.env.LITEGEN_MODELS_DIR ?? join(REPO, 'models'),
      definitions: await loadDefinitions(join(here, 'providers')),
      baselineDir,
      outDir,
      only,
      quotes: !values['no-quotes'],
      knownFile: join(here, 'quotes-known.txt'),
      maxLines,
    });
    const after = gitStatus(baselineDir);
    let guard = '';
    if (before !== null && after !== before) {
      guard = `\nWARNING: git status of scripts/model-drift/snapshots changed during the run:\nbefore:\n${before || '(clean)\n'}after:\n${after || '(clean)\n'}`;
      r.exitCode = Math.max(r.exitCode, 2);
    }
    process.stdout.write(r.summary + guard);
    process.exitCode = r.exitCode;
  } catch (e) {
    if (e.usage) {
      // A provider the catalog does not know (selectProviders), refused before anything was written.
      process.stderr.write(`pipeline: ${e.message}\n${USAGE}`);
      process.exitCode = 2;
    } else {
      process.stderr.write(
        `PIPELINE BROKEN: ${e.stack ?? e.message}\n\n` +
          'This is a bug in scripts/model-drift itself, not a vendor change: fix the script, then run\n' +
          "  node --test 'scripts/model-drift/test/*.test.mjs'\n" +
          `and rerun the pipeline. Partial outputs, if any, are in ${outDir}.\n`,
      );
      process.exitCode = 3;
    }
  }
}
