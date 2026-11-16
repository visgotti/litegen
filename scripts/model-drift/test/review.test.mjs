// review.mjs: the review packet compares snapshots section by section, so a
// reader sees what changed upstream and nothing that only moved.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { sections, jsonDiff, lineDiff, reviewProvider, formatReview } from '../review.mjs';

/** Two snapshot trees, { 'p/file.txt': text } each. */
function trees(baseline, fresh) {
  const mk = (files) => {
    const dir = mkdtempSync(join(tmpdir(), 'drift-review-'));
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(join(dir, path.split('/')[0]), { recursive: true });
      writeFileSync(join(dir, path), text);
    }
    return dir;
  };
  return { baselineDir: mk(baseline), freshDir: mk(fresh) };
}

const schema = (props) => JSON.stringify({ 'POST /v1/x (application/json)': { properties: props } }, null, 2);

test('sections split a snapshot at its headings and match by heading up to the status suffix', () => {
  const s = sections('top line\n## fal-ai/a — active text-to-image\nbody a\n## fal-ai/b — deprecated text-to-image\nbody b\n');
  assert.deepEqual(s.map((x) => x.id), ['(top)', '## fal-ai/a', '## fal-ai/b']);
  assert.equal(s[1].text, 'body a');
  assert.equal(s[1].start, 2);
});

test('jsonDiff reports an enum gaining a value as one line, and ignores a reorder', () => {
  const a = { duration: { enum: [5, 10] }, ratio: { enum: ['16:9', '9:16'] } };
  const b = { duration: { enum: [5, 10, 15] }, ratio: { enum: ['9:16', '16:9'] } };
  assert.deepEqual(jsonDiff(a, b), ['+ duration.enum[] 15']);
});

test('jsonDiff matches union branches by their model const, so a new branch is one added line', () => {
  const branch = (model, max) => ({ properties: { model: { const: model }, duration: { maximum: max } } });
  const a = { oneOf: [branch('gen4', 10), branch('gen4.5', 10)] };
  const b = { oneOf: [branch('aleph', 5), branch('gen4', 15), branch('gen4.5', 10)] };
  assert.deepEqual(jsonDiff(a, b), [
    '+ oneOf[model=aleph] = {"properties":{"model":{"const":"aleph"},"duration":{"maximum":5}}}',
    '~ oneOf[model=gen4].properties.duration.maximum: 10 → 15',
  ]);
});

test('lineDiff lists removed then added lines, as multisets', () => {
  assert.deepEqual(lineDiff(['| a | 1 |', '| b | 2 |', '| b | 2 |'], ['| a | 1 |', '| b | 2 |', '| c | 3 |']), ['- | b | 2 |', '+ | c | 3 |']);
});

test('a section that moved to another file of the provider compares with its old self, so an unchanged move is silent', () => {
  const ep = (id, max) => `## ${id} — active text-to-image\n${schema({ steps: { maximum: max } })}\n`;
  const { baselineDir, freshDir } = trees(
    { 'fal/carried-image.txt': ep('fal-ai/a', 50) + ep('fal-ai/b', 50) },
    { 'fal/carried-image.txt': ep('fal-ai/a', 50), 'fal/carried-image-2.txt': ep('fal-ai/b', 50) },
  );
  const r = reviewProvider(baselineDir, freshDir, 'fal');
  assert.deepEqual(r.items, []);
  assert.deepEqual(r.files.map((f) => [f.file, f.status]), [['carried-image-2.txt', 'new'], ['carried-image.txt', 'changed']]);
});

test('a moved section that also changed shows only its changed paths, and where it came from', () => {
  const ep = (id, max) => `## ${id} — active text-to-image\n${schema({ steps: { maximum: max } })}\n`;
  const { baselineDir, freshDir } = trees(
    { 'fal/carried-image.txt': ep('fal-ai/a', 50) + ep('fal-ai/b', 50) },
    { 'fal/carried-image.txt': ep('fal-ai/a', 50), 'fal/carried-image-2.txt': ep('fal-ai/b', 40) },
  );
  const [item] = reviewProvider(baselineDir, freshDir, 'fal').items;
  assert.equal(item.kind, 'changed');
  assert.equal(item.from, 'carried-image.txt');
  assert.deepEqual(item.lines, ['~ POST /v1/x (application/json).properties.steps.maximum: 50 → 40']);
});

test('a section with no counterpart in the baseline is new and printed whole, and a status change reads as a heading change', () => {
  const { baselineDir, freshDir } = trees(
    { 'fal/carried-image.txt': '## fal-ai/a — active text-to-image\n(no openapi)\n' },
    { 'fal/carried-image.txt': '## fal-ai/a — not in the model registry\n(no openapi)\n## fal-ai/c — active text-to-image\nfresh schema\n' },
  );
  const r = reviewProvider(baselineDir, freshDir, 'fal');
  assert.deepEqual(r.items.map((i) => [i.kind, i.id]), [['changed', '## fal-ai/a'], ['new', '## fal-ai/c']]);
  assert.deepEqual(r.items[0].lines, ['~ heading: ## fal-ai/a — active text-to-image → ## fal-ai/a — not in the model registry']);
  const text = formatReview([r], { freshDir });
  assert.match(text, /## new, read whole: ## fal-ai\/c — active text-to-image/);
  assert.match(text, /^ {2}fresh schema$/m);
});

test('a heading unique in one tree but repeated in the other compares file by file, so it reads as a diff, not as gone plus new', () => {
  // kling's "# Models" heads both capability-image.txt and capability-video.txt
  // today: once one tree has a heading twice, both trees key it by file.
  const items = (t) => reviewProvider(t.baselineDir, t.freshDir, 'acme').items.map((i) => [i.kind, i.file, i.lines]);
  const repeatedInFresh = trees(
    { 'acme/a.txt': '## Models\nalpha\n' },
    { 'acme/a.txt': '## Models\nalpha\nbeta\n', 'acme/b.txt': '## Models\ngamma\n' },
  );
  assert.deepEqual(items(repeatedInFresh), [['changed', 'a.txt', ['+ beta']], ['new', 'b.txt', ['## Models', 'gamma']]]);
  const repeatedInBaseline = trees(
    { 'acme/a.txt': '## Models\nalpha\n', 'acme/b.txt': '## Models\ngamma\n' },
    { 'acme/a.txt': '## Models\nalpha\nbeta\n' },
  );
  assert.deepEqual(items(repeatedInBaseline), [['changed', 'a.txt', ['+ beta']], ['gone', 'b.txt', []]]);
});

test('a baseline section no fresh file holds is reported gone', () => {
  const { baselineDir, freshDir } = trees(
    { 'acme/docs.txt': '## page-a\nx\n## page-b\ny\n' },
    { 'acme/docs.txt': '## page-a\nx\n' },
  );
  assert.deepEqual(reviewProvider(baselineDir, freshDir, 'acme').items.map((i) => [i.kind, i.id]), [['gone', '## page-b']]);
});

test('formatReview caps a long section and says how to read the rest', () => {
  const body = Array.from({ length: 10 }, (_, i) => `row ${i}`).join('\n');
  const { baselineDir, freshDir } = trees({}, { 'acme/docs.txt': `## page\n${body}\n` });
  const text = formatReview([reviewProvider(baselineDir, freshDir, 'acme')], { maxLines: 3, freshDir });
  assert.match(text, /row 2\n {2}… 7 more lines: sed -n '1,11p'/);
});

test('review.mjs refuses a --max-lines that is not a whole number, instead of printing every section with no lines', () => {
  const REVIEW = join(dirname(fileURLToPath(import.meta.url)), '..', 'review.mjs');
  const { baselineDir, freshDir } = trees({ 'acme/docs.txt': '## page\nrow 0\n' }, { 'acme/docs.txt': '## page\nrow 1\n' });
  const cli = (...args) => spawnSync(process.execPath, [REVIEW, '--fresh', freshDir, '--baseline', baselineDir, ...args], { encoding: 'utf8' });
  for (const args of [['--max-lines', 'abc'], ['--max-lines', ''], ['--max-lines=-1'], ['--max-lines', '2.5']]) {
    const r = cli(...args);
    assert.equal(r.status, 2, `${args.join(' ')}: ${r.stdout}`);
    assert.equal(r.stdout, '', args.join(' '));
    assert.match(r.stderr, /--max-lines must be a whole number[\s\S]*usage: /, args.join(' '));
  }
  const ok = cli('--max-lines', '1');
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /^ {2}- row 0\n {2}… 1 more lines/m);
});
