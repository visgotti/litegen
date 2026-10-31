// A shutdown the vendor's own docs announce reads as `missing`, not as a
// model still listed (Google/OpenAI audit D1–D3, 2026-10-30).
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { shutDownIds, allStrings } from '../extract.mjs';
import openai from '../providers/openai.mjs';

test('shutDownIds reads the ids a page says were shut down, not their replacements', () => {
  const veo =
    'Veo 3.1 Preview (`veo-3.1-generate-preview`, `veo-3.1-fast-generate-preview`, and `veo-3.1-lite-generate-preview`) in the Gemini API was [shut down](/gemini-api/docs/deprecations) on **October 22, 2026** . Migrate to [Gemini Omni Flash](/x) (`gemini-omni-1.1-flash`) or the GA models.';
  assert.deepEqual(shutDownIds(veo).sort(), ['veo-3.1-fast-generate-preview', 'veo-3.1-generate-preview', 'veo-3.1-lite-generate-preview']);
});

test('shutDownIds reads a spec description, sentence by sentence', () => {
  const prose = 'The model to use. Defaults to `gpt-image-2`. `gpt-image-1` was shut down on October 23, 2026. Use `gpt-image-2.5-sunburst` or `gpt-image-2.5-flare` instead.';
  assert.deepEqual(shutDownIds(prose), ['gpt-image-1']);
});

test('shutDownIds ignores a shutdown still to come', () => {
  assert.deepEqual(shutDownIds('`gpt-image-1-mini` will be shut down on December 1, 2026.'), []);
  assert.deepEqual(shutDownIds('`a` has been shut down. `b` remains.'), ['a']);
});

test('allStrings collects every string in a parsed document', () => {
  assert.deepEqual(allStrings({ a: 'x', b: [1, 'y', { c: 'z' }] }).sort(), ['x', 'y', 'z']);
});

test('the OpenAI spec does not list a model its own prose says was shut down', () => {
  const spec = {
    paths: {
      '/images/generations': { post: { requestBody: { content: { 'application/json': { schema: { properties: { model: {
        description: '`gpt-image-1` was shut down on October 23, 2026. Use `gpt-image-2` instead.',
        enum: ['gpt-image-2', 'gpt-image-1'],
      } } } } } } } },
      '/images/edits': { post: { requestBody: { content: { 'multipart/form-data': { schema: { properties: { model: { enum: ['gpt-image-2', 'gpt-image-1'] } } } } } } } },
    },
  };
  const openapi = openai.sources.find((s) => s.key === 'openapi');
  assert.deepEqual(openapi.extract(spec), ['gpt-image-2']);
});
