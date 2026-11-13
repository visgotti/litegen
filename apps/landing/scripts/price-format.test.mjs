import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTranslator } from 'next-intl';
import * as fmt from '../src/lib/price-format.mjs';
import { readCatalog } from './catalog.mjs';
import { deriveModel } from './derive-models.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

/** The page's own translator for the `reference` messages, collecting next-intl's errors. */
function translator(locale, errors = []) {
  const messages = JSON.parse(readFileSync(join(ROOT, 'messages', `${locale}.json`), 'utf8'));
  return createTranslator({ locale, messages, namespace: 'reference', onError: (e) => errors.push(`${e.code}: ${e.message}`) });
}

/** The USD amounts a row's price shows: its base and every rule amount in dollars. */
function shippedUsdAmounts() {
  const out = [];
  for (const row of readCatalog()) {
    if (row.pricing?.base_cost_usd !== undefined) out.push([row.id, row.pricing.base_cost_usd]);
    for (const rule of row.pricing?.variable_pricing ?? []) {
      for (const f of ['usd', 'per_second_usd', 'per_megapixel_usd', 'per_item_usd', 'add_usd']) {
        if (typeof rule[f] === 'number') out.push([`${row.id} ${f}`, rule[f]]);
      }
    }
  }
  return out;
}

/** A printed amount read back as a number: test-side parsing, not the code under test. */
const readBack = {
  en: (s) => Number(s.replace('$', '').replaceAll(',', '')),
  es: (s) => Number(s.replace(' US$', '').replaceAll('.', '').replace(',', '.')),
};

test('an amount shows every digit the catalog gives it', () => {
  const cases = {
    en: [
      [0.003, '$0.003'],
      [0.0035, '$0.0035'],
      [0.015, '$0.015'],
      [0.045, '$0.045'],
      [7.5, '$7.50'],
      [0.2113536, '$0.2113536'],
      [0.1 * 5, '$0.50'],
      // Binary noise is not a digit the catalog gave.
      [0.1 * 3, '$0.30'],
      [2, '$2.00'],
      [1234.5, '$1,234.50'],
    ],
    // es puts a no-break space before US$.
    es: [
      [0.003, '0,003 US$'],
      [7.5, '7,50 US$'],
    ],
  };
  for (const [locale, rows] of Object.entries(cases)) {
    for (const [usd, want] of rows) assert.equal(fmt.formatUsd(usd, locale), want, `${locale} ${usd}`);
  }
});

test('every shipped price prints back to itself', () => {
  const amounts = shippedUsdAmounts();
  assert.ok(amounts.length > 276, `expected every base and rule amount, got ${amounts.length}`);
  for (const locale of ['en', 'es']) {
    const wrong = amounts
      .filter(([, usd]) => readBack[locale](fmt.formatUsd(usd, locale)) !== usd)
      .map(([what, usd]) => `${what}: ${usd} prints ${fmt.formatUsd(usd, locale)}`);
    assert.deepEqual(wrong, [], `${locale}: ${wrong.length} amounts do not print back`);
  }
});

test('a price rule reads as the engine applies it', () => {
  const en = translator('en');
  const es = translator('es');
  const cases = [
    [en, 'en', { when: [], effect: { kind: 'perSecond', value: 1.5 } }, 'always: $1.50 per second'],
    [en, 'en', { when: [{ fact: 'resolution', is: 'eq', value: '540p' }], effect: { kind: 'perSecond', value: 0.75 } }, 'resolution = 540p: $0.75 per second'],
    [en, 'en', { when: [{ fact: 'references', is: 'range', gte: 1 }], effect: { kind: 'perMegapixel', value: 0.01 } }, 'references ≥ 1: $0.01 per megapixel'],
    [en, 'en', { when: [{ fact: 'audio', is: 'eq', value: true }], effect: { kind: 'addUsd', value: 0.1 } }, 'audio = true: + $0.10'],
    [en, 'en', { when: [], effect: { kind: 'perItem', value: 0.06, list: 'animations' } }, 'always: $0.06 per entry in animations'],
    [en, 'en', { when: [], effect: { kind: 'multiplyPerMegapixelAbove', value: 1.048576 } }, 'always: × megapixels ÷ 1.048576, above 1.048576 MP'],
    // vidu/viduq2-turbo's shipped rule lowers the price: never "+ -$0.01".
    [en, 'en', { when: [], effect: { kind: 'addUsd', value: -0.01 } }, 'always: − $0.01'],
    // A factor keeps every digit (a default Intl format would print 7.112).
    [en, 'en', { when: [{ fact: 'quality', is: 'eq', value: 'high' }], effect: { kind: 'multiply', value: 7.111617 } }, 'quality = high: × 7.111617'],
    [en, 'en', { when: [{ fact: 'references', is: 'sent' }], effect: { kind: 'usd', value: 0.3 } }, 'references sent: $0.30'],
    [en, 'en', { when: [{ fact: 'size', is: 'sent' }, { fact: 'megapixels', is: 'range', gt: 1.048576, lte: 4.194304 }], effect: { kind: 'usd', value: 0.7 } },
      'size sent and 1.048576 < megapixels ≤ 4.194304: $0.70'],
    [en, 'en', { when: [{ fact: 'duration_seconds', is: 'eq', value: 5 }], effect: { kind: 'usd', value: 0.1 } }, 'duration_seconds = 5: $0.10'],
    [es, 'es', { when: [{ fact: 'resolution', is: 'eq', value: '540p' }], effect: { kind: 'perSecond', value: 0.75 } }, 'resolution = 540p: 0,75 US$ por segundo'],
    [es, 'es', { when: [], effect: { kind: 'addUsd', value: -0.01 } }, 'siempre: − 0,01 US$'],
    [es, 'es', { when: [{ fact: 'size', is: 'sent' }, { fact: 'megapixels', is: 'range', gt: 1.048576, lte: 4.194304 }], effect: { kind: 'usd', value: 0.7 } },
      'size enviado y 1,048576 < megapixels ≤ 4,194304: 0,70 US$'],
  ];
  for (const [t, locale, rule, want] of cases) assert.equal(fmt.ruleText(rule, t, locale), want);
});

test('a head names the price and what one charge buys', () => {
  const pricing = { baseCostUsd: 0.003, unit: 'image', rules: [] };
  assert.equal(fmt.priceText(pricing, translator('en'), 'en'), '$0.003 / image');
  assert.equal(fmt.priceText(pricing, translator('es'), 'es'), '0,003 US$ / imagen');
  assert.equal(fmt.priceText({ baseCostUsd: 0.06, unit: 'model3d', rules: [] }, translator('en'), 'en'), '$0.06 / 3D model');
  assert.equal(fmt.priceText({ baseCostUsd: 0.005, unit: 'request', rules: [] }, translator('en'), 'en'), '$0.005 / request');
});

test('the default request is worded the way the conformance test builds it', () => {
  // catalog_conformance.rs every_base_price_is_what_the_rules_charge_for_the_default_request:
  // every param at its default, one clip on an animate model, keyframes at 0 s and 5 s on a keyframe model.
  const en = translator('en');
  const entry = (pricing, more = {}) => ({ pricing, params: [], refRoles: [], ...more });
  assert.equal(
    fmt.priceDefaultText(entry({ baseCostUsd: 7.5, unit: 'video', rules: [] }), en, 'en'),
    '$7.50 / video for the default request: every parameter at its default.',
  );
  assert.equal(
    fmt.priceDefaultText(entry({ baseCostUsd: 0.06, unit: 'model3d', rules: [] }, { params: [{ name: 'animations', kind: 'animation_list' }] }), en, 'en'),
    '$0.06 / 3D model for the default request: every parameter at its default, and one clip.',
  );
  assert.equal(
    fmt.priceDefaultText(entry({ baseCostUsd: 0.2, unit: 'video', rules: [] }, { refRoles: [{ name: 'keyframe', required: true, min: 2, max: 31 }] }), en, 'en'),
    '$0.20 / video for the default request: every parameter at its default, and keyframes at 0 s and 5 s.',
  );
  assert.equal(
    fmt.priceDefaultText(entry({ baseCostUsd: 7.5, unit: 'video', rules: [] }), translator('es'), 'es'),
    '7,50 US$ / vídeo con la petición por defecto: cada parámetro en su valor por defecto.',
  );
});

test('every number a rule shows prints back to itself', () => {
  const numbers = [];
  for (const row of readCatalog()) {
    for (const rule of row.pricing?.variable_pricing ?? []) {
      for (const f of ['multiply', 'multiply_per_megapixel_above']) if (typeof rule[f] === 'number') numbers.push([`${row.id} ${f}`, rule[f]]);
      for (const [fact, want] of Object.entries(rule.when ?? {})) {
        if (typeof want === 'number') numbers.push([`${row.id} ${fact}`, want]);
        else if (want && typeof want === 'object') for (const [b, n] of Object.entries(want)) numbers.push([`${row.id} ${fact}.${b}`, n]);
      }
    }
  }
  assert.ok(numbers.some(([, n]) => n === 7.111617) && numbers.some(([, n]) => n === 1.048576), 'the shipped factors and anchors are covered');
  const readNumber = {
    en: (s) => Number(s.replaceAll(',', '')),
    es: (s) => Number(s.replaceAll('.', '').replace(',', '.')),
  };
  for (const locale of ['en', 'es']) {
    const wrong = numbers
      .filter(([, n]) => readNumber[locale](fmt.formatNumber(n, locale)) !== n)
      .map(([what, n]) => `${what}: ${n} prints ${fmt.formatNumber(n, locale)}`);
    assert.deepEqual(wrong, [], `${locale}: ${wrong.length} numbers do not print back`);
  }
});

test('every shipped price rule words cleanly in both locales', () => {
  for (const locale of ['en', 'es']) {
    const errors = [];
    const t = translator(locale, errors);
    for (const row of readCatalog()) {
      const entry = deriveModel(row);
      for (const rule of entry.pricing?.rules ?? []) {
        const text = fmt.ruleText(rule, t, locale);
        assert.doesNotMatch(text, /reference\.|undefined|NaN/, `${row.id}: ${text}`);
      }
      if (entry.pricing) fmt.priceDefaultText(entry, t, locale);
    }
    assert.deepEqual(errors, [], `${locale}: next-intl reported ${errors.length} errors`);
  }
});
