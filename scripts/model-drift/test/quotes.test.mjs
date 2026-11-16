// quotes.mjs: every quoted vendor string in models/*.yaml must still be on a
// page its comment block cites, so a vendor's price or rule change shows up.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { normalize, pageText, extractQuotes, fragments, verify, parseKnown, fetchPages, checkQuotes, formatQuotes } from '../quotes.mjs';

const YAML = [
  'models:',
  '  # Price: "$0.17 per second at 720p … $0.29 per second at 1080p".',
  '  # @see https://vendor.test/pricing.md (2026-11-16)',
  '  - id: acme/clip-1',
  '    provider: acme',
  '    media_type: video',
  '    params:',
  '      # "Maximum length of **2000 characters**" on img2video.',
  '      # @see <https://vendor.test/api.json#img2video> (2026-11-16)',
  '      prompt_max: 2000',
  '      # A comment with "quoted words" but no page cited is not checked.',
  '      seed: { kind: seed }',
  '',
].join('\n');

test('extractQuotes reads each quote with the pages its block cites and the row it belongs to', () => {
  const q = extractQuotes(YAML, 'models/acme.yaml');
  assert.deepEqual(q.map((x) => [x.model, x.line, x.urls]), [
    ['acme/clip-1', 2, ['https://vendor.test/pricing.md']],
    ['acme/clip-1', 8, ['https://vendor.test/api.json']],
  ]);
  assert.deepEqual(q[0].fragments, ['$0.17 per second at 720p', '$0.29 per second at 1080p']);
});

test('fragments unwrap a link elided as (…), split at any other ellipsis, and drop pieces too short to mean anything', () => {
  assert.deepEqual(fragments('| [V4.1 Flash V2](…) | \\$0.007 | — |'), ['v4.1 flash v2 $0.007']);
  assert.deepEqual(fragments('$0.17 per second … $0.29 per second'), ['$0.17 per second', '$0.29 per second']);
  assert.deepEqual(fragments('1K … 2K'), []);
});

test('normalize makes markdown, escapes, dashes and curly quotes compare equal', () => {
  assert.equal(normalize('Maximum length of **2000   characters**'), normalize('maximum length of 2000 characters'));
  assert.equal(normalize('`gpt-image-2` — “deprecated”'), 'gpt-image-2 - "deprecated"');
  assert.equal(normalize('\\$0.007 per image'), '$0.007 per image');
});

test('a quoted table row matches the table on the page, pipes, backticks and links aside', () => {
  const page = pageText('| Model | Price |\n| --- | --- |\n| [`gpt-image-2`](https://x.test/m) | **Low** $0.006 |\n| 768P | $0.06 |');
  for (const quote of ['gpt-image-2 | Low $0.006', 'Low $0.006', '768p $0.06', '| [gpt-image-2](…) | Low $0.006 |']) {
    for (const f of fragments(quote)) assert.ok(page.includes(f), `${quote} → ${f} in ${page}`);
  }
});

test('inline HTML tags leave no stray space before punctuation, and block tags still separate words', () => {
  const page = pageText('<html><body><p><code>gpt-image-2</code>, <code>gpt-image-1</code>, or <code>gpt-image-1-mini</code></p><table><tr><td>Price</td><td>$0.17</td></tr></table></body></html>', 'text/html');
  assert.ok(page.includes(normalize('gpt-image-2, gpt-image-1, or gpt-image-1-mini')), page);
  assert.ok(page.includes('price $0.17'), page);
});

test('an elision written as […] or (…) splits a quote like an ellipsis, and stray brackets at a fragment edge are dropped', () => {
  assert.deepEqual(fragments('text-to-video […], image-to-video […]'), ['text-to-video', 'image-to-video']);
  assert.deepEqual(fragments('the model map (…) says 5 s'), ['the model map', 'says 5 s']);
});

test('a quote wrapped across comment lines matches: no space between CJK characters, none after a hyphen at a break', () => {
  assert.equal(normalize('可包含正向与负向 描述'), normalize('可包含正向与负向描述'));
  assert.equal(normalize('succeeded（生成 成功）'), normalize('succeeded（生成成功）'));
  assert.equal(normalize('bedrock- runtime'), normalize('bedrock-runtime'));
  assert.equal(normalize('720p - 1080p'), '720p - 1080p');
});

test('Markdoc tags inside page text ({% sup /%}) do not split a quoted price', () => {
  const page = pageText('equivalent to $0.067 per 1K image{% sup /%}, $0.101 per 2K image{% sup /%}, and $0.151 per 4K image');
  assert.ok(page.includes(normalize('$0.067 per 1K image, $0.101 per 2K image, and $0.151 per 4K image')), page);
});

test('HTML inside a markdown page (MDX tables) is read as text', () => {
  const page = pageText('## Costs\n<table><tr><td>Low</td><td>$0.006</td></tr><tr><td><code>medium</code></td><td>$0.053</td></tr></table>', 'text/markdown');
  assert.ok(page.includes('low $0.006') && page.includes('medium $0.053'), page);
});

test('code comment markers at line starts do not split a quoted sentence', () => {
  const page = pageText('\t// 不传：随机种子生成。\n\t// 正数：固定种子生成。\n');
  assert.ok(page.includes(normalize('不传：随机种子生成。 正数：固定种子生成。')), page);
});

test('fetchPages also reads the markdown twin of an HTML docs page, for pages whose text is rendered by script', async () => {
  const pages = await fetchPages(['https://docs.test/pricing'], {
    gapMs: 0,
    fetchImpl: async (url) =>
      url.endsWith('.md')
        ? { status: 200, contentType: 'text/markdown', body: '| Low | $0.006 |' }
        : { status: 200, contentType: 'text/html', body: '<!doctype html><html><div id="root"></div></html>' },
  });
  assert.ok(pages.get('https://docs.test/pricing').text.includes('low $0.006'));
});

test('pageText reads JSON as its strings and HTML without its tags or entities', () => {
  assert.match(pageText('{"description":"Maximum length of 2000 characters\\nmore"}', 'application/json'), /maximum length of 2000 characters more/);
  assert.match(pageText('<html><body><p>$0.17 per second&nbsp;at 720p</p><script>x()</script></body></html>', 'text/html'), /\$0\.17 per second at 720p/);
});

test('verify finds a quote on any of its pages, and reports one whose words are gone', () => {
  const quotes = extractQuotes(YAML, 'models/acme.yaml');
  const pages = new Map([
    ['https://vendor.test/pricing.md', { ok: true, text: pageText('Now $0.20 per second at 720p and $0.29 per second at 1080p.') }],
    ['https://vendor.test/api.json', { ok: true, text: pageText('{"prompt":{"description":"Maximum length of 2000 characters"}}', 'application/json') }],
  ]);
  const r = verify(quotes, pages);
  assert.equal(r.found.length, 1);
  assert.deepEqual(r.missing.map((m) => [m.model, m.absent]), [['acme/clip-1', ['$0.17 per second at 720p']]]);
});

test('verify leaves the quotes of a failed page unchecked and lists the page as failed', () => {
  const quotes = extractQuotes(YAML, 'models/acme.yaml');
  const pages = new Map([
    ['https://vendor.test/pricing.md', { ok: false, error: 'HTTP 404' }],
    ['https://vendor.test/api.json', { ok: true, text: 'maximum length of 2000 characters' }],
  ]);
  const r = verify(quotes, pages);
  assert.equal(r.unchecked.length, 1);
  assert.deepEqual(r.failedUrls, [{ url: 'https://vendor.test/pricing.md', error: 'HTTP 404' }]);
});

test('a quote in the known file is not reported, and a known line that no longer applies is stale', () => {
  const quotes = extractQuotes(YAML, 'models/acme.yaml');
  const pages = new Map([
    ['https://vendor.test/pricing.md', { ok: true, text: '$0.29 per second at 1080p' }],
    ['https://vendor.test/api.json', { ok: true, text: 'maximum length of 2000 characters' }],
  ]);
  const known = parseKnown('# header\nacme/clip-1 | $0.17 per second at 720p  # JS-rendered table\nacme/gone | some old words here\n');
  const r = verify(quotes, pages, known);
  assert.equal(r.missing.length, 0);
  assert.equal(r.known.length, 1);
  assert.deepEqual(r.stale.map((s) => s.model), ['acme/gone']);
});

test('a known-quotes comment needs two spaces before its #, so a fragment may hold a # of its own', () => {
  assert.deepEqual(parseKnown('acme/x | ## response  # the heading is markup\n'), [{ model: 'acme/x', fragment: '## response' }]);
});

test('a quote above the first row belongs to the file header, named for the provider', () => {
  const q = extractQuotes('models:\n  # "Every price below is per image."\n  # @see https://vendor.test/pricing.md\n\n  - id: acme/a\n', 'models/acme.yaml');
  assert.equal(q[0].model, 'acme/(header)');
});

test('fetchPages fetches each url once and turns an HTTP error into a failed page', async () => {
  const seen = [];
  const pages = await fetchPages(['https://a.test/x', 'https://a.test/y', 'https://b.test/z'], {
    gapMs: 0,
    fetchImpl: async (url) => {
      seen.push(url);
      return url.endsWith('/y') ? { status: 404, body: 'nope' } : { status: 200, contentType: 'text/plain', body: `page ${url}` };
    },
  });
  assert.deepEqual(seen.sort(), ['https://a.test/x', 'https://a.test/y', 'https://b.test/z']);
  assert.deepEqual(pages.get('https://a.test/y'), { ok: false, error: 'HTTP 404' });
  assert.equal(pages.get('https://b.test/z').ok, true);
});

test('checkQuotes exits 1 when a quote changed, 2 when a cited page failed, 0 when all hold', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'drift-quotes-'));
  writeFileSync(join(dir, 'acme.yaml'), YAML);
  const run = (answers) => checkQuotes({ modelsDir: dir, fetchImpl: async (url) => answers[url] });
  const good = {
    'https://vendor.test/pricing.md': { status: 200, contentType: 'text/markdown', body: '$0.17 per second at 720p; $0.29 per second at 1080p' },
    'https://vendor.test/api.json': { status: 200, contentType: 'application/json', body: '{"d":"Maximum length of 2000 characters"}' },
  };
  assert.equal((await run(good)).exitCode, 0);
  const changed = await run({ ...good, 'https://vendor.test/pricing.md': { status: 200, contentType: 'text/markdown', body: '$0.20 per second at 720p; $0.29 per second at 1080p' } });
  assert.equal(changed.exitCode, 1);
  assert.match(formatQuotes(changed), /missing: "\$0\.17 per second at 720p"/);
  const dead = await run({ ...good, 'https://vendor.test/api.json': { status: 404, body: '' } });
  assert.equal(dead.exitCode, 2);
  assert.match(formatQuotes(dead), /HTTP 404 · https:\/\/vendor\.test\/api\.json/);
});
