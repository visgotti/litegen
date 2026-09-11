// Shared extractors used by provider definitions.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { openapiModelIds, openapiExcerpt, markdownExcerpt, yamlPrune, parseYaml } from '../extract.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const spec = JSON.parse(readFileSync(join(here, 'fixtures', 'openapi-mini.json'), 'utf8'));

test('openapiModelIds collects model const/enum values from the named endpoints only, through $ref and anyOf', () => {
  assert.deepEqual(openapiModelIds(spec, { paths: ['/v1/gen', '/v1/edit'] }).sort(), ['alpha', 'beta', 'beta-fast', 'gamma', 'gamma-mini']);
});

test('openapiModelIds fails loudly when a named endpoint is gone from the spec', () => {
  assert.throws(() => openapiModelIds(spec, { paths: ['/v1/gen', '/v1/removed'] }), /\/v1\/removed.*not in the spec/);
});

test('openapiExcerpt keeps only the discriminated branches for models we carry', () => {
  const text = openapiExcerpt(spec, { paths: ['/v1/gen'], ours: ['beta'] });
  assert.match(text, /"maximum": 10/);
  assert.doesNotMatch(text, /"1:1"/);
});

test('openapiExcerpt keeps a non-discriminated schema whole and drops prose keys that churn', () => {
  const text = openapiExcerpt(spec, { paths: ['/v1/edit', '/v1/gen'], ours: ['gamma', 'alpha'] });
  assert.match(text, /"1024x1024"/);
  assert.match(text, /"1:1"/);
  assert.doesNotMatch(text, /prose that changes weekly|"example"|"title"/);
});

test('openapiExcerpt is byte-identical for the same spec with keys in a different order', () => {
  const shuffled = JSON.parse(JSON.stringify(spec, (k, v) =>
    v && typeof v === 'object' && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).reverse()) : v));
  assert.equal(openapiExcerpt(shuffled, { paths: ['/v1/gen'], ours: ['alpha', 'beta'] }), openapiExcerpt(spec, { paths: ['/v1/gen'], ours: ['alpha', 'beta'] }));
});

test('markdownExcerpt keeps the fenced spec blocks of a docs page and drops the surrounding prose', () => {
  const md = [
    '# Text to video', 'Some marketing prose that gets rewritten.', '',
    '```yaml', 'model:', '  enum: [MiniMax-Hailuo-2.3]', '```', '',
    'More prose.', '```json', '{"duration": 6}', '```',
  ].join('\n');
  assert.equal(markdownExcerpt(md), 'model:\n  enum: [MiniMax-Hailuo-2.3]\n\n{"duration": 6}\n');
});

test('markdownExcerpt honours longer fences, so a ``` line inside a ````yaml block does not end it', () => {
  const md = ['````yaml spec.json POST /v1/x', 'a: 1', '```', 'b: 2', '````', 'prose'].join('\n');
  assert.equal(markdownExcerpt(md), 'a: 1\n```\nb: 2\n');
});

test('markdownExcerpt can keep only fences of one language', () => {
  const md = ['```bash', 'curl x', '```', '```yaml', 'model: m', '```'].join('\n');
  assert.equal(markdownExcerpt(md, { lang: 'yaml' }), 'model: m\n');
});

test('yamlPrune drops prose keys with their folded or indented continuation, keeping structure', () => {
  const yaml = [
    'model:',
    '  type: string',
    '  description: >-',
    '    Model name. Supported values:',
    '    `A`, `B`.',
    '  enum:',
    '    - A',
    '    - B',
    'duration:',
    '  description: "one line"',
    '  example: 6',
    '  enum: [6, 10]',
  ].join('\n');
  assert.equal(yamlPrune(yaml), ['model:', '  type: string', '  enum:', '    - A', '    - B', 'duration:', '  enum: [6, 10]', ''].join('\n'));
});

test('yamlPrune keeps a request property that is itself named description', () => {
  const yaml = ['properties:', '  description:', '    type: string', '  model:', '    type: string'].join('\n');
  assert.equal(yamlPrune(yaml), yaml + '\n');
});

// ─── parseYaml (block-style subset, for vendors that publish only YAML specs) ──

test('parseYaml reads nested block maps, sequences and typed scalars', () => {
  const yaml = [
    'openapi: 3.1.0',
    'paths:',
    '  /v1/images/generations:',
    '    post:',
    '      parameters:',
    '        - name: Content-Type',
    '          required: true',
    '      requestBody:',
    '        content:',
    '          application/json:',
    '            schema:',
    '              properties:',
    '                model:',
    '                  enum:',
    '                    - recraftv3',
    "                    - 'recraftv4_1'",
    '                    - "recraftv3_vector"',
    '                n: { }',
    '                size:',
    '                  default: 1024x1024',
    '                  maximum: 6',
    '                  nullable: null',
  ].join('\n').replace('n: { }', 'n: {}');
  assert.deepEqual(parseYaml(yaml), {
    openapi: '3.1.0',
    paths: { '/v1/images/generations': { post: {
      parameters: [{ name: 'Content-Type', required: true }],
      requestBody: { content: { 'application/json': { schema: { properties: {
        model: { enum: ['recraftv3', 'recraftv4_1', 'recraftv3_vector'] },
        n: {},
        size: { default: '1024x1024', maximum: 6, nullable: null },
      } } } } },
    } } },
  });
});

test('parseYaml consumes a block scalar whole and unescapes quoted scalars', () => {
  const yaml = ['description: >-', '  Model name.', '  Supported values below.', 'title: "a \\"quoted\\" word"', "note: 'it''s'"].join('\n');
  const doc = parseYaml(yaml);
  assert.match(doc.description, /^Model name\.\s+Supported values below\.$/);
  assert.equal(doc.title, 'a "quoted" word');
  assert.equal(doc.note, "it's");
});

test('parseYaml refuses flow collections and anchors instead of guessing', () => {
  assert.throws(() => parseYaml('enum: [a, b]'), /unsupported YAML syntax/);
  assert.throws(() => parseYaml('base: &anchor\n  a: 1'), /unsupported YAML syntax/);
});
