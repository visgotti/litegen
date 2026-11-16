#!/usr/bin/env node
// Quoted vendor strings in models/*.yaml, checked against the pages they cite.
//
// Catalog comments quote the vendor verbatim beside `# @see <url> (date)`:
// prices ("$0.17 per second at 720p"), bounds, enum lists, deprecation rows.
// When a vendor changes a price or a rule, its page stops containing the old
// words. This script fetches every cited page once and checks that each quote
// in a comment block still appears on one of the pages that block cites, so a
// price or contract change shows up without anyone re-reading the pages.
//
//   node scripts/model-drift/quotes.mjs [--provider a,b] [--json] [--known file]
//
// A comment block is a run of full-line comments; its quotes are the "…"
// (or “…”) spans in it, its pages the `@see` URLs in it. A quote is split at
// an ellipsis (… or ...), and each fragment of 8 or more characters must
// appear on one of the block's pages, compared after normalizing both sides
// (case, whitespace, markdown emphasis and backticks, backslash escapes,
// dashes and curly quotes; HTML tags and entities; JSON as its strings).
//
// Exit codes, as check.mjs: 0 every quote found (or listed in the known
// file) · 1 a quote is no longer on its pages (the vendor changed it: read the
// page, then fix the row and the quote) · 2 a cited page failed (moved or
// dead: find where it went and fix the `@see`), or a bad command line.
//
// The known file (default scripts/model-drift/quotes-known.txt) lists quotes a
// script cannot verify, one per line: `<model id> | <fragment>  # why`. A
// fragment found again, or a row gone, is reported as stale so the list
// shrinks.
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { httpFetch } from './lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const MIN_FRAGMENT = 8;

// ─── Normalizing ─────────────────────────────────────────────────────────────

const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', mdash: '—', ndash: '–', hellip: '…', rsquo: '’', lsquo: '‘', ldquo: '“', rdquo: '”', times: '×' };
const decodeEntities = (s) =>
  s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (m, e) => {
    if (e[0] === '#') return String.fromCodePoint(e[1].toLowerCase() === 'x' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10));
    return ENTITIES[e.toLowerCase()] ?? m;
  });

/** A markdown link as its text: `[text](url)` → `text` (the url is not prose). */
const unlink = (s) => String(s).replace(/\[([^\]]*)\]\([^)]*\)/g, '$1');

/**
 * The comparable form of a quote or a page: lower case, plain punctuation,
 * single spaces; markdown emphasis, backticks, links and table pipes gone, and
 * no space before closing punctuation (inline HTML tags leave one).
 */
export function normalize(s) {
  return unlink(s)
    // Markdoc / Liquid tags ({% sup /%}) are markup, not prose.
    .replace(/\{%[^%]*%\}/g, '')
    .toLowerCase()
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[–—−]/g, '-')
    .replace(/ /g, ' ')
    .replace(/\\([\\`*_{}[\]()#+\-.!|$<>~])/g, '$1')
    .replace(/[`*|]/g, ' ')
    .replace(/\s+/g, ' ')
    .replace(/ ([,.;:!?)\]])/g, '$1')
    .replace(/([([]) /g, '$1')
    // A comment wraps lines anywhere: CJK text has no spaces between
    // characters, and a word hyphenated at a line break has none after it.
    .replace(new RegExp(`(${CJK}) (?=${CJK})`, 'gu'), '$1')
    .replace(/(\w)- (?=\w)/g, '$1-')
    .trim();
}

const CJK = '[\\u3000-\\u303f\\u3400-\\u9fff\\uff00-\\uffef]';

/** Every string value in a JSON document, as one text. */
function jsonStrings(doc) {
  const out = [];
  const visit = (n) => {
    if (typeof n === 'string') out.push(n);
    else if (Array.isArray(n)) n.forEach(visit);
    else if (n && typeof n === 'object') {
      for (const [k, v] of Object.entries(n)) {
        out.push(k);
        visit(v);
      }
    }
  };
  visit(doc);
  return out.join('\n');
}

/** A fetched page as searchable text: JSON as its keys and strings, HTML without tags, the rest as is. */
export function pageText(body, contentType = '') {
  const text = String(body ?? '');
  const trimmed = text.trimStart();
  if (/json/i.test(contentType) || trimmed.startsWith('{') || trimmed.startsWith('[')) {
    try {
      return normalize(`${jsonStrings(JSON.parse(text))}\n${text}`);
    } catch {
      // not JSON after all
    }
  }
  if (/html/i.test(contentType) || /^<(!doctype|html)/i.test(trimmed)) {
    // Keep the raw text too: some pages carry their content in script data.
    return normalize(`${decodeEntities(untag(text))}\n${decodeEntities(text)}`);
  }
  // Markdown can embed HTML (MDX tables); source files (an SDK's doc
  // comments) start lines with comment markers, which are not prose.
  const plain = text.replace(/^[ \t]*(?:\/\/+|\*|#)[ \t]?/gm, '');
  return normalize(/<\/?[a-z][^>]*>/i.test(plain) ? `${decodeEntities(untag(plain))}\n${plain}` : plain);
}

/** HTML as text: scripts and styles dropped, block tags as spaces, inline tags (code, strong, a, span) as nothing. */
function untag(html) {
  return html
    .replace(/<script[\s\S]*?<\/script>/gi, ' ')
    .replace(/<style[\s\S]*?<\/style>/gi, ' ')
    .replace(/<\/?(?:p|div|br|li|ul|ol|tr|td|th|table|thead|tbody|h[1-6]|section|article|pre|blockquote|dd|dt|dl|figure|figcaption)\b[^>]*>/gi, ' ')
    .replace(/<[^>]+>/g, '');
}

// ─── Reading the catalog ─────────────────────────────────────────────────────

/** A cited URL as fetched: no fragment, no trailing punctuation or angle brackets. */
export const fetchUrl = (u) => u.replace(/^<|>$/g, '').replace(/[),.;:'"]+$/, '').replace(/#.*$/, '');

/**
 * The fragments of a quote worth checking: links elided as `[text](…)`
 * unwrapped, split at an ellipsis (`…`, `...`, `[…]`, `(…)`), normalized,
 * stray brackets and separators trimmed from each end, at least MIN_FRAGMENT
 * characters.
 */
export const fragments = (quote) =>
  unlink(quote.replace(/\]\(…\)/g, '](x)'))
    .split(/\[…\]|\(…\)|\[\.\.\.\]|\(\.\.\.\)|…|\.\.\./)
    .map((f) => normalize(f).replace(/^[\s\])}>,;:.·-]+/, '').replace(/[\s[({<,;:·-]+$/, ''))
    .filter((f) => f.length >= MIN_FRAGMENT);

/**
 * Every quote in a catalog file's comment blocks: { file, line, model, urls,
 * quote, fragments }. `model` is the row the block belongs to: the row that
 * starts right after a block above a `- id:` line, else the row it sits in.
 */
export function extractQuotes(text, file = '') {
  const lines = text.split('\n');
  const out = [];
  // Above the first row, a block belongs to the file header.
  let model = `${file.replace(/^.*\//, '').replace(/\.ya?ml$/, '') || 'catalog'}/(header)`;
  for (let i = 0; i < lines.length; i++) {
    const id = lines[i].match(/^\s*-\s+id:\s*(\S+)/);
    if (id) {
      model = id[1].replace(/^["']|["']$/g, '');
      continue;
    }
    if (!/^\s*#/.test(lines[i])) continue;
    const start = i;
    const block = [];
    while (i < lines.length && /^\s*#/.test(lines[i])) block.push(lines[i++].replace(/^\s*#\s?/, ''));
    const next = lines[i]?.match(/^\s*-\s+id:\s*(\S+)/);
    const owner = next ? next[1].replace(/^["']|["']$/g, '') : model;
    i--; // the outer loop's i++ lands on the line after the block
    const joined = block.join(' ');
    const urls = [...new Set([...joined.matchAll(/@see\s+<?(https:\/\/[^\s>]+)/g)].map((m) => fetchUrl(m[1])))];
    if (!urls.length) continue;
    for (const m of joined.matchAll(/"([^"]{8,}?)"|“([^”]{8,}?)”/g)) {
      const quote = m[1] ?? m[2];
      const frags = fragments(quote);
      if (frags.length) out.push({ file, line: start + 1, model: owner, urls, quote, fragments: frags });
    }
  }
  return out;
}

// ─── Checking ────────────────────────────────────────────────────────────────

/**
 * Parse the known file: `<model> | <fragment>  # why` lines. The comment
 * needs two spaces before its `#` (a normalized fragment never holds two),
 * so a fragment may contain a `#` of its own.
 */
export function parseKnown(text) {
  return String(text)
    .split('\n')
    .map((l) => l.replace(/ {2,}#.*$/, '').trim())
    .filter((l) => l && !l.startsWith('#'))
    .map((l) => {
      const at = l.indexOf(' | ');
      if (at < 0) throw new Error(`known-quotes line has no " | ": ${l}`);
      return { model: l.slice(0, at).trim(), fragment: normalize(l.slice(at + 3)) };
    });
}

/**
 * Check every quote against its block's pages.
 * @param pages Map url → { ok, text?, error? }
 * @returns {{ found, missing, unchecked, known, stale, failedUrls }}
 */
export function verify(quotes, pages, known = []) {
  const knownKey = new Set(known.map((k) => `${k.model} | ${k.fragment}`));
  const used = new Set();
  const result = { found: [], missing: [], unchecked: [], known: [], stale: [], failedUrls: [] };
  for (const q of quotes) {
    const texts = q.urls.map((u) => pages.get(u)).filter((p) => p?.ok).map((p) => p.text);
    if (!texts.length) {
      result.unchecked.push(q);
      continue;
    }
    const absent = q.fragments.filter((f) => !texts.some((t) => t.includes(f)));
    if (!absent.length) {
      for (const f of q.fragments) if (knownKey.has(`${q.model} | ${f}`)) result.stale.push({ model: q.model, fragment: f, why: 'found again' });
      result.found.push(q);
      continue;
    }
    const unknown = absent.filter((f) => !knownKey.has(`${q.model} | ${f}`));
    absent.filter((f) => knownKey.has(`${q.model} | ${f}`)).forEach((f) => used.add(`${q.model} | ${f}`));
    if (unknown.length) result.missing.push({ ...q, absent: unknown });
    else result.known.push(q);
  }
  const quoted = new Set(quotes.flatMap((q) => q.fragments.map((f) => `${q.model} | ${f}`)));
  for (const k of known) {
    const key = `${k.model} | ${k.fragment}`;
    if (!quoted.has(key)) result.stale.push({ ...k, why: 'no such quote in the catalog any more' });
  }
  for (const [url, p] of pages) if (!p.ok) result.failedUrls.push({ url, error: p.error });
  return result;
}

/** Fetch every url once, one host at a time per host (vendors rate-limit bursts), `hosts` hosts at once. */
export async function fetchPages(urls, { fetchImpl = httpFetch, hosts = 6, gapMs = 300 } = {}) {
  const byHost = new Map();
  for (const u of urls) {
    const host = new URL(u).host;
    if (!byHost.has(host)) byHost.set(host, []);
    byHost.get(host).push(u);
  }
  const pages = new Map();
  const queue = [...byHost.values()];
  const worker = async () => {
    while (queue.length) {
      const list = queue.shift();
      for (const [n, url] of list.entries()) {
        if (n > 0 && gapMs) await new Promise((r) => setTimeout(r, gapMs));
        try {
          const res = await fetchImpl(url, { timeoutMs: 60_000 });
          if (res.status >= 400) {
            pages.set(url, { ok: false, error: `HTTP ${res.status}` });
            continue;
          }
          let text = pageText(res.body, res.contentType);
          // An HTML docs page with no extension often has a markdown twin
          // (`<page>.md`) holding the text its script renders.
          const html = /html/i.test(res.contentType ?? '') || /^\s*<(!doctype|html)/i.test(res.body ?? '');
          if (html && !/\.[a-z0-9]+$/i.test(new URL(url).pathname)) {
            const twin = new URL(url);
            twin.pathname = twin.pathname.replace(/\/$/, '') + '.md';
            try {
              const md = await fetchImpl(twin.href, { timeoutMs: 60_000 });
              if (md.status < 400 && !/^\s*<(!doctype|html)/i.test(md.body ?? '')) text += `\n${pageText(md.body, md.contentType)}`;
            } catch {
              // no twin: the HTML is all there is
            }
          }
          pages.set(url, { ok: true, text });
        } catch (e) {
          pages.set(url, { ok: false, error: `fetch failed: ${e.message}` });
        }
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(hosts, queue.length) }, worker));
  return pages;
}

/** Every quote in models/*.yaml (mock skipped), optionally only some providers' files. */
export function catalogQuotes(modelsDir, only) {
  return readdirSync(modelsDir)
    .filter((f) => f.endsWith('.yaml') && f !== 'mock.yaml')
    .filter((f) => !only || only.includes(f.replace(/\.yaml$/, '')))
    .sort()
    .flatMap((f) => extractQuotes(readFileSync(join(modelsDir, f), 'utf8'), `models/${f}`));
}

export async function checkQuotes({ modelsDir, only, knownFile, fetchImpl }) {
  const quotes = catalogQuotes(modelsDir, only);
  const known = knownFile && existsSync(knownFile) ? parseKnown(readFileSync(knownFile, 'utf8')) : [];
  const pages = await fetchPages([...new Set(quotes.flatMap((q) => q.urls))], { fetchImpl });
  const result = verify(quotes, pages, known.filter((k) => !only || only.includes(k.model.split('/')[0])));
  const exitCode = result.failedUrls.length ? 2 : result.missing.length ? 1 : 0;
  return { exitCode, quotes: quotes.length, pages: pages.size, ...result };
}

const short = (s, n = 140) => (s.length > n ? `${s.slice(0, n)}…` : s);

/** Human report: what changed, what is dead, and what to do about each. */
export function formatQuotes(r) {
  const lines = [`Quoted vendor strings · ${r.quotes} quotes on ${r.pages} pages · exit ${r.exitCode}`];
  lines.push(`found ${r.found.length} · not on their pages ${r.missing.length} · known unverifiable ${r.known.length} · unchecked (page failed) ${r.unchecked.length}`);
  if (r.failedUrls.length) {
    lines.push('', 'Cited pages that failed (fix the `@see` in models/*.yaml: find where the page moved via the vendor llms.txt or docs index; the quotes they back are unchecked):');
    for (const f of r.failedUrls) lines.push(`  ${f.error} · ${f.url}`);
  }
  if (r.missing.length) {
    lines.push('', 'Quotes no longer on their pages (the vendor changed the text: read the page, then fix the row, its price or bound, and the quote):');
    for (const m of r.missing) {
      lines.push(`  ${m.model ?? '(file header)'} · ${m.file}:${m.line}`);
      for (const f of m.absent) lines.push(`    missing: "${short(f)}"`);
      lines.push(`    pages: ${m.urls.join(' ')}`);
    }
  }
  if (r.stale.length) {
    lines.push('', 'Stale lines in the known-quotes file (remove them):');
    for (const s of r.stale) lines.push(`  ${s.model} | ${short(s.fragment)}  (${s.why})`);
  }
  return lines.join('\n') + '\n';
}

// ─── CLI ─────────────────────────────────────────────────────────────────────

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let values;
  try {
    ({ values } = parseArgs({
      args: process.argv.slice(2),
      strict: true,
      allowPositionals: false,
      options: { provider: { type: 'string', multiple: true }, json: { type: 'boolean' }, known: { type: 'string' } },
    }));
  } catch (e) {
    process.stderr.write(`quotes: ${e.message}\nusage: node scripts/model-drift/quotes.mjs [--provider a,b] [--json] [--known file]\n`);
    process.exit(2);
  }
  const only = values.provider?.flatMap((v) => v.split(',')).map((s) => s.trim()).filter(Boolean);
  const r = await checkQuotes({
    modelsDir: process.env.LITEGEN_MODELS_DIR ?? join(here, '..', '..', 'models'),
    only,
    knownFile: values.known ?? join(here, 'quotes-known.txt'),
  });
  process.stdout.write(values.json ? JSON.stringify(r, null, 2) + '\n' : formatQuotes(r));
  process.exitCode = r.exitCode;
}
