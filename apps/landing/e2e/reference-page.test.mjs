/**
 * The built reference page, read as a visitor gets it: the static export's
 * `out/{en,es}/reference.html`. Each model row's head (the collapsed row,
 * which the export renders for every model) must show the catalog's price
 * exactly, with what one charge buys, and say what the model does.
 *
 * Run after a build: `npm run build && npm run test:built`. A missing page
 * fails, never skips (CI's no-skips.sh forbids skipping). The export folder is
 * `out` (where `next build` exports with no NEXT_DIST_DIR, or with
 * NEXT_DIST_DIR=out); LANDING_EXPORT_DIR names another one, relative to
 * apps/landing.
 *
 * The details (rules, params, lifecycle) render only on click, so they are
 * covered by the derivation and wording tests in scripts/, not here.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readCatalog } from '../scripts/catalog.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const EXPORT = join(ROOT, process.env.LANDING_EXPORT_DIR || 'out');

const ENTITIES = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#x27;': "'", '&#39;': "'" };

/** Every row head of one locale's page, by model id, as plain text. */
function heads(locale) {
  const file = join(EXPORT, locale, 'reference.html');
  assert.ok(existsSync(file), `${file} is missing: run \`npm run build\` first`);
  const html = readFileSync(file, 'utf8').replaceAll('<!-- -->', '');
  const out = new Map();
  for (const [, id, rest] of html.matchAll(/<code class="ModelsReference_id__[^"]*">([^<]+)<\/code>([\s\S]*?)<\/button>/g)) {
    const text = rest
      .replace(/<[^>]+>/g, ' ')
      .replace(/&(?:amp|lt|gt|quot|#x27|#39);/g, (e) => ENTITIES[e])
      // Collapse layout whitespace only: a no-break space is part of a price.
      .replace(/[ \t\r\n]+/g, ' ')
      .trim();
    out.set(id, text);
  }
  assert.ok(out.size > 0, `no model rows found in ${file}`);
  return out;
}

/** A price token as the page prints it, read back as a number (test-side parsing). */
const PRICE = {
  en: { re: /\$([\d,]+\.\d+)/, num: (s) => Number(s.replaceAll(',', '')) },
  es: { re: /([\d.]+,\d+) US\$/, num: (s) => Number(s.replaceAll('.', '').replace(',', '.')) },
};

test('a sub-cent price shows every digit, with its unit', () => {
  const en = heads('en');
  const want = [
    ['fal/flux-schnell', '$0.003 / image'],
    ['minimax/image-01', '$0.0035 / image'],
    ['bfl/flux-2-klein-9b', '$0.015 / image'],
    ['fal/flux-2-pro-outpaint', '$0.045 / image'],
    ['bedrock/luma.ray-v2:0', '$7.50 / video · varies'],
  ];
  for (const [id, text] of want) {
    assert.ok(en.get(id)?.includes(text), `${id}: want "${text}" in the head, got "${en.get(id)}"`);
  }
  const es = heads('es');
  assert.ok(
    es.get('fal/flux-schnell')?.includes('0,003 US$ / imagen'),
    `es fal/flux-schnell: got "${es.get('fal/flux-schnell')}"`,
  );
});

test('every row head shows its catalog price exactly', () => {
  for (const locale of ['en', 'es']) {
    const page = heads(locale);
    const wrong = [];
    for (const row of readCatalog()) {
      const head = page.get(row.id);
      const token = head?.match(PRICE[locale].re);
      const shown = token ? PRICE[locale].num(token[1]) : undefined;
      if (shown !== row.pricing?.base_cost_usd) wrong.push(`${row.id}: ${token?.[0] ?? head ?? '(no row)'} != ${row.pricing?.base_cost_usd}`);
    }
    assert.deepEqual(wrong, [], `${locale}: ${wrong.length} heads differ from base_cost_usd`);
  }
});

test('a head says the price varies exactly when the row has price rules', () => {
  const page = heads('en');
  const varies = [...page].filter(([, text]) => text.includes(' · varies')).map(([id]) => id).sort();
  const ruled = readCatalog()
    .filter((row) => (row.pricing?.variable_pricing ?? []).length > 0)
    .map((row) => row.id)
    .sort();
  assert.deepEqual(varies, ruled);
});

test('every row head says what the model does', () => {
  const want = {
    en: [
      ['fal/trellis', 'image→3D'],
      ['fal/hyper3d-rodin', 'text→3D'],
      ['fal/tripo-h3.1-multiview', 'multiview→3D'],
      ['fal/bria-rmbg-2', 'background removal'],
      ['pixellab/pixflux', 'pixel art'],
      ['google/gemini-3.1-flash-image', 'deprecated'],
    ],
    es: [
      ['fal/trellis', 'imagen→3D'],
      ['fal/bria-rmbg-2', 'quitar fondo'],
      ['google/gemini-3.1-flash-image', 'obsoleto'],
    ],
  };
  for (const [locale, rows] of Object.entries(want)) {
    const page = heads(locale);
    for (const [id, text] of rows) {
      assert.ok(page.get(id)?.includes(text), `${locale} ${id}: want "${text}" in the head, got "${page.get(id)}"`);
    }
  }
});
