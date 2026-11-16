#!/usr/bin/env node
// Review packet for a drift run: what really changed between the committed
// snapshots (the reviewed baseline) and the ones a run just wrote. Raw `diff`
// over snapshots is expensive to read: fal regroups endpoints between chunk
// files, a discriminated union gains a branch and every later index shifts,
// and a source added since the last commit has no baseline file at all. This
// compares by section instead:
//  - a section is a markdown heading and the lines under it (the text before
//    the first heading is the file's top section); it is matched across every
//    file of the provider by its heading, up to " — " (fal's "## <endpoint> —
//    <status>"), so a section that moved to another file compares with its old
//    self, and a status change reads as a changed heading (a heading either
//    tree repeats, kling's "# Models" say, matches within its own file);
//  - a JSON block inside a section (openapiExcerpt output) is diffed by
//    structure: enum values as sets, oneOf/anyOf branches matched by their
//    `model` const, so only changed paths are printed;
//  - any other text is diffed by lines (a multiset diff, in order of
//    appearance), capped per section.
// A section with no counterpart in the baseline is printed whole up to the
// cap (it must be read whole), with its file and line range.
//
//   node scripts/model-drift/review.mjs --fresh <dir> [--baseline scripts/model-drift/snapshots] [--provider a,b] [--max-lines 60]
//
// Exit 0 always (it is a report); 2 on a bad command line.
import { readdirSync, readFileSync, existsSync, statSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const here = dirname(fileURLToPath(import.meta.url));

// ─── Sections ────────────────────────────────────────────────────────────────

const HEADING = /^#{1,6} \S/;
/** The identity a section is matched by: its heading up to " — " (fal's status suffix). */
export const sectionId = (heading) => heading.replace(/\s+—\s+.*$/, '').trim();

/**
 * A snapshot file as sections: { id, heading, text, start, end } (1-based lines).
 * The lines before the first heading are the top section, id "(top)".
 */
export function sections(text) {
  const lines = text.replace(/\n$/, '').split('\n');
  const out = [];
  let cur = { id: '(top)', heading: '', lines: [], start: 1 };
  lines.forEach((line, i) => {
    if (HEADING.test(line)) {
      out.push(cur);
      cur = { id: sectionId(line), heading: line, lines: [], start: i + 1 };
    } else {
      cur.lines.push(line);
    }
  });
  out.push(cur);
  return out
    .filter((s) => s.heading || s.lines.some((l) => l.trim()))
    .map((s) => ({ id: s.id, heading: s.heading, text: s.lines.join('\n'), start: s.start, end: s.start + s.lines.length }));
}

/**
 * A section's body split into JSON blocks and text: a line that is exactly
 * `{` opens a block that ends at the next line that is exactly `}`. A block
 * that does not parse stays text.
 */
export function parts(body) {
  const lines = body.split('\n');
  const json = [];
  const text = [];
  for (let i = 0; i < lines.length; i++) {
    if (lines[i] === '{') {
      const end = lines.indexOf('}', i);
      if (end > i) {
        try {
          json.push(JSON.parse(lines.slice(i, end + 1).join('\n')));
          i = end;
          continue;
        } catch {
          // not JSON after all: keep it as text
        }
      }
    }
    text.push(lines[i]);
  }
  return { json, text: text.filter((l) => l.trim() !== '') };
}

// ─── Structural JSON diff ────────────────────────────────────────────────────

const isObj = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const compact = (v, max = 160) => {
  const s = JSON.stringify(v);
  return s.length > max ? `${s.slice(0, max)}…` : s;
};

/** What identifies an element of a union or list: its `model` const or enum, else its const, type or $ref. */
function discriminator(e) {
  if (!isObj(e)) return undefined;
  const model = e.properties?.model;
  if (model?.const !== undefined) return `model=${model.const}`;
  if (Array.isArray(model?.enum)) return `model=${model.enum.join('|')}`;
  if (e.const !== undefined) return `const=${e.const}`;
  if (typeof e.$ref === 'string') return e.$ref;
  if (Array.isArray(e.enum)) return `enum:${typeof e.enum[0]}`;
  if (typeof e.type === 'string' && Object.keys(e).length <= 3) return `type=${e.type}`;
  return undefined;
}

function keyedBy(arr) {
  const keys = arr.map(discriminator);
  if (keys.some((k) => k === undefined) || new Set(keys).size !== keys.length) return null;
  return new Map(arr.map((e, i) => [keys[i], e]));
}

/**
 * The paths where two JSON values differ, as lines: `~ path: old → new`,
 * `+ path = value`, `- path (was value)`. Arrays of primitives compare as
 * sets (an enum gaining a value is one line; a reorder is nothing); arrays of
 * objects match by discriminator when every element has a distinct one, else
 * by index.
 */
export function jsonDiff(a, b, path = '', out = []) {
  if (isObj(a) && isObj(b)) {
    for (const k of [...new Set([...Object.keys(a), ...Object.keys(b)])].sort()) {
      const p = path ? `${path}.${k}` : k;
      if (!(k in b)) out.push(`- ${p} (was ${compact(a[k])})`);
      else if (!(k in a)) out.push(`+ ${p} = ${compact(b[k])}`);
      else jsonDiff(a[k], b[k], p, out);
    }
    return out;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    const primitive = (arr) => arr.every((v) => v === null || typeof v !== 'object');
    if (primitive(a) && primitive(b)) {
      const sa = new Set(a.map((v) => JSON.stringify(v)));
      const sb = new Set(b.map((v) => JSON.stringify(v)));
      const added = [...sb].filter((v) => !sa.has(v));
      const removed = [...sa].filter((v) => !sb.has(v));
      if (added.length) out.push(`+ ${path}[] ${added.join(', ')}`);
      if (removed.length) out.push(`- ${path}[] ${removed.join(', ')}`);
      return out;
    }
    const ka = keyedBy(a);
    const kb = keyedBy(b);
    if (ka && kb) {
      for (const k of [...new Set([...ka.keys(), ...kb.keys()])].sort()) {
        const p = `${path}[${k}]`;
        if (!kb.has(k)) out.push(`- ${p} (was ${compact(ka.get(k))})`);
        else if (!ka.has(k)) out.push(`+ ${p} = ${compact(kb.get(k))}`);
        else jsonDiff(ka.get(k), kb.get(k), p, out);
      }
      return out;
    }
    const n = Math.max(a.length, b.length);
    for (let i = 0; i < n; i++) {
      const p = `${path}[${i}]`;
      if (i >= b.length) out.push(`- ${p} (was ${compact(a[i])})`);
      else if (i >= a.length) out.push(`+ ${p} = ${compact(b[i])}`);
      else jsonDiff(a[i], b[i], p, out);
    }
    return out;
  }
  if (JSON.stringify(a) !== JSON.stringify(b)) out.push(`~ ${path || '(value)'}: ${compact(a)} → ${compact(b)}`);
  return out;
}

/** Lines in `a` but not `b` (`- `) and in `b` but not `a` (`+ `), as multisets, in order of appearance. */
export function lineDiff(a, b) {
  const count = (lines) => lines.reduce((m, l) => m.set(l, (m.get(l) ?? 0) + 1), new Map());
  const left = count(b);
  const removed = [];
  for (const l of a) {
    if (left.get(l) > 0) left.set(l, left.get(l) - 1);
    else removed.push(`- ${l}`);
  }
  const right = count(a);
  const added = [];
  for (const l of b) {
    if (right.get(l) > 0) right.set(l, right.get(l) - 1);
    else added.push(`+ ${l}`);
  }
  return [...removed, ...added];
}

/** The change lines between two versions of one section (empty when nothing changed). */
export function sectionDiff(before, after) {
  const out = [];
  if (before.heading !== after.heading) out.push(`~ heading: ${before.heading} → ${after.heading}`);
  const a = parts(before.text);
  const b = parts(after.text);
  const blocks = Math.max(a.json.length, b.json.length);
  for (let i = 0; i < blocks; i++) {
    const tag = blocks > 1 ? `json#${i + 1} ` : '';
    if (i >= b.json.length) out.push(`- ${tag}(a JSON block of ${Object.keys(a.json[i]).length} keys is gone)`);
    else if (i >= a.json.length) out.push(`+ ${tag}= ${compact(b.json[i], 400)}`);
    else out.push(...jsonDiff(a.json[i], b.json[i]).map((l) => (tag ? l.replace(/^(.) /, `$1 ${tag}`) : l)));
  }
  out.push(...lineDiff(a.text, b.text));
  return out;
}

// ─── One provider ────────────────────────────────────────────────────────────

const txtFiles = (dir) => (existsSync(dir) && statSync(dir).isDirectory() ? readdirSync(dir).filter((f) => f.endsWith('.txt')).sort() : []);

/**
 * Compare one provider's snapshot folder in the baseline with the fresh one.
 * @returns {{ provider, files: { file, status: 'changed'|'new'|'removed'|'same' }[], items: { file, kind, id, heading, start, end, lines }[] }}
 *   kind: 'changed' (a section whose content differs) · 'new' (no counterpart anywhere in the
 *   baseline: read it whole) · 'gone' (a baseline section no fresh file has).
 */
export function reviewProvider(baselineDir, freshDir, provider) {
  const read = (dir, f) => readFileSync(join(dir, provider, f), 'utf8');
  const before = txtFiles(join(baselineDir, provider));
  const after = txtFiles(join(freshDir, provider));
  const files = [...new Set([...before, ...after])].sort().map((file) => {
    if (!after.includes(file)) return { file, status: 'removed' };
    if (!before.includes(file)) return { file, status: 'new' };
    return { file, status: read(baselineDir, file) === read(freshDir, file) ? 'same' : 'changed' };
  });
  // Every section of each tree, keyed by id, so a section that moved to another
  // file still meets its old self. An id either tree uses twice keeps its file
  // in the key in both trees: keyed by id in one tree and by file in the other,
  // the same section would read as gone plus new.
  const all = (dir, list) => list.flatMap((file) => sections(read(dir, file)).map((s) => ({ ...s, file })));
  const oldSections = all(baselineDir, before);
  const curSections = all(freshDir, after);
  const repeated = new Set();
  for (const tree of [oldSections, curSections]) {
    const seen = new Set();
    for (const s of tree) (seen.has(s.id) ? repeated : seen).add(s.id);
  }
  const key = (s) => (s.id === '(top)' || repeated.has(s.id) ? `${s.file}::${s.id}` : s.id);
  const old = new Map(oldSections.map((s) => [key(s), s]));
  const cur = new Map(curSections.map((s) => [key(s), s]));
  const items = [];
  for (const [key, s] of cur) {
    const file = files.find((f) => f.file === s.file);
    if (file.status === 'same') {
      old.delete(key);
      continue;
    }
    const prev = old.get(key);
    old.delete(key);
    if (!prev) {
      items.push({ file: s.file, kind: 'new', id: s.id, heading: s.heading, start: s.start, end: s.end, lines: [s.heading, ...s.text.split('\n')].filter(Boolean) });
      continue;
    }
    const lines = sectionDiff(prev, s);
    if (lines.length) items.push({ file: s.file, kind: 'changed', id: s.id, heading: s.heading, start: s.start, end: s.end, lines, from: prev.file === s.file ? undefined : prev.file });
  }
  for (const [, s] of old) {
    if (files.find((f) => f.file === s.file)?.status === 'same') continue;
    items.push({ file: s.file, kind: 'gone', id: s.id, heading: s.heading, start: s.start, end: s.end, lines: [] });
  }
  return { provider, files, items };
}

/** The packet as text, each section's lines capped at `maxLines`. */
export function formatReview(results, { maxLines = 60, freshDir = '<fresh>' } = {}) {
  const out = [];
  for (const r of results) {
    const touched = r.files.filter((f) => f.status !== 'same');
    if (!touched.length) continue;
    out.push(`# ${r.provider}`);
    out.push(`files: ${touched.map((f) => `${f.file} (${f.status})`).join(', ')}`);
    if (!r.items.length) out.push('(every section is unchanged: the files differ only in where sections sit or in blank lines)');
    for (const it of r.items) {
      const where = `${it.file}:${it.start}-${it.end}`;
      const label = it.id === '(top)' ? '(top of file)' : it.heading;
      if (it.kind === 'gone') {
        out.push(`## gone: ${label} (was in ${it.file})`);
        continue;
      }
      const from = it.from ? ` (moved from ${it.from})` : '';
      out.push(`## ${it.kind === 'new' ? 'new, read whole' : 'changed'}: ${label}${from} · ${join(freshDir, r.provider, where)}`);
      const shown = it.kind === 'new' ? it.lines.slice(it.id === '(top)' ? 0 : 1) : it.lines;
      out.push(...shown.slice(0, maxLines).map((l) => `  ${l}`));
      if (shown.length > maxLines) out.push(`  … ${shown.length - maxLines} more lines: sed -n '${it.start},${it.end}p' '${join(freshDir, r.provider, it.file)}'`);
    }
    out.push('');
  }
  return out.length ? out.join('\n') : 'No snapshot differs from the baseline.\n';
}

/**
 * The `--max-lines` value (the cap per section; 60 when the flag is left out):
 * a whole number, 0 or more. Anything else throws, for the command line to
 * refuse with its usage line, since Number("abc") is NaN and a NaN cap prints
 * every section with no lines and no hint that any were cut.
 */
export function parseMaxLines(value, fallback = 60) {
  if (value === undefined) return fallback;
  if (!/^\d+$/.test(value)) throw new Error(`--max-lines must be a whole number of lines (0 or more), not ${JSON.stringify(value)}`);
  return Number(value);
}

/** Every provider folder in either tree, or the ones named. */
export function review({ baselineDir, freshDir, only }) {
  const dirs = (d) => (existsSync(d) ? readdirSync(d).filter((f) => statSync(join(d, f)).isDirectory()) : []);
  const providers = [...new Set([...dirs(baselineDir), ...dirs(freshDir)])].filter((p) => !only || only.includes(p)).sort();
  return providers.map((p) => reviewProvider(baselineDir, freshDir, p));
}

// ─── CLI ─────────────────────────────────────────────────────────────────────

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let values;
  let maxLines;
  try {
    ({ values } = parseArgs({
      args: process.argv.slice(2),
      strict: true,
      allowPositionals: false,
      options: {
        fresh: { type: 'string' },
        baseline: { type: 'string' },
        provider: { type: 'string', multiple: true },
        'max-lines': { type: 'string' },
      },
    }));
    if (!values.fresh) throw new Error('--fresh <dir> is required: the snapshots folder a run wrote (check.mjs --snapshots)');
    maxLines = parseMaxLines(values['max-lines']);
  } catch (e) {
    process.stderr.write(`review: ${e.message}\nusage: node scripts/model-drift/review.mjs --fresh <dir> [--baseline dir] [--provider a,b] [--max-lines 60]\n`);
    process.exit(2);
  }
  const only = values.provider?.flatMap((v) => v.split(',')).map((s) => s.trim()).filter(Boolean);
  const freshDir = resolve(values.fresh);
  const results = review({ baselineDir: resolve(values.baseline ?? join(here, 'snapshots')), freshDir, only });
  process.stdout.write(formatReview(results, { maxLines, freshDir }));
}
