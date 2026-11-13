/**
 * Guards for the page's messages. next-intl renders a missing or unparseable
 * message as its key path and the build still succeeds, so these catch what
 * the build would not: a key added in one locale only, and a message that does
 * not parse (an ICU message holding an angle bracket is read as a tag).
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTranslator } from 'next-intl';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const LOCALES = ['en', 'es'];
const messages = Object.fromEntries(LOCALES.map((l) => [l, JSON.parse(readFileSync(join(ROOT, 'messages', `${l}.json`), 'utf8'))]));

/** Every leaf message as [dotted key path, text]. */
function leaves(obj, prefix = '') {
  return Object.entries(obj).flatMap(([k, v]) =>
    v !== null && typeof v === 'object' ? leaves(v, `${prefix}${k}.`) : [[`${prefix}${k}`, v]],
  );
}

test('en and es carry the same message keys', () => {
  const [en, es] = LOCALES.map((l) => leaves(messages[l]).map(([k]) => k).sort());
  assert.deepEqual(
    { onlyEn: en.filter((k) => !es.includes(k)), onlyEs: es.filter((k) => !en.includes(k)) },
    { onlyEn: [], onlyEs: [] },
  );
});

test('every reference message parses and formats in both locales', () => {
  for (const locale of LOCALES) {
    const errors = [];
    const t = createTranslator({
      locale,
      messages: messages[locale],
      namespace: 'reference',
      onError: (e) => errors.push(`${e.code}: ${e.message}`),
    });
    for (const [key, text] of leaves(messages[locale].reference)) {
      // A number for every placeholder: it fills a plain {x} and a plural alike.
      const values = Object.fromEntries([...String(text).matchAll(/\{(\w+)/g)].map(([, name]) => [name, 1]));
      const out = t(key, values);
      assert.notEqual(out, `reference.${key}`, `${locale} reference.${key} rendered as its key path`);
    }
    assert.deepEqual(errors, [], `${locale}: next-intl reported ${errors.length} errors`);
  }
});

test('no reference message holds an angle bracket', () => {
  for (const locale of LOCALES) {
    const bad = leaves(messages[locale].reference).filter(([, text]) => /[<>]/.test(String(text))).map(([k]) => k);
    assert.deepEqual(bad, [], `${locale}: an ICU message with < or > is read as a tag`);
  }
});
