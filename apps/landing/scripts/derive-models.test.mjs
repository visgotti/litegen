import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { parse } from 'yaml';
import { deriveModel, deriveModels } from './derive-models.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const MODELS_DIR = join(HERE, '..', '..', '..', 'models');

test('deriveModel maps DALL-E 3 flags, sizes, and params', () => {
  const e = deriveModel({
    id: 'openai/dall-e-3',
    provider: 'openai',
    media_type: 'image',
    display_name: 'DALL-E 3',
    description: 'x',
    pricing: { base_cost_usd: 0.04 },
    capabilities: { text_to_image: true },
    prompt: { required: true, max_length: 4000 },
    params: {
      size: { kind: 'size', mode: 'enum', values: [[1024, 1024], [1792, 1024], [1024, 1792]] },
      quality: { kind: 'string', enum_values: ['standard', 'hd'], default: 'standard' },
    },
    tags: ['text-to-image'],
  });

  assert.equal(e.id, 'openai/dall-e-3');
  assert.equal(e.output, 'image');
  assert.deepEqual(e.sizes, ['1024x1024', '1792x1024', '1024x1792']);
  assert.equal(e.capabilities.textToImage, true);
  assert.equal(e.capabilities.imageToImage, false);
  assert.equal(e.maxRefImages, 0);
  assert.equal(e.pricing.baseCostUsd, 0.04);
  assert.equal(e.promptLimits.maxLength, 4000);

  const quality = e.params.find((p) => p.name === 'quality');
  assert.deepEqual(quality.enum, ['standard', 'hd']);
  assert.equal(quality.default, 'standard');
  // size + aspect_ratio are surfaced separately, not in params.
  assert.equal(e.params.find((p) => p.name === 'size'), undefined);
});

test('deriveModel maps ref inputs + video capabilities', () => {
  const e = deriveModel({
    id: 'openai/sora',
    provider: 'openai',
    media_type: 'video',
    display_name: 'Sora 2',
    capabilities: { text_to_video: true, image_to_video: true },
    prompt: { required: true, max_length: 4000 },
    params: { aspect_ratio: { kind: 'aspect_ratio', allowed: ['16:9', '9:16'], default: '16:9' } },
    ref_inputs: { max_total: 1, roles: { init: { required: false, min_count: 0, max_count: 1 } } },
    tags: ['text-to-video'],
  });

  assert.equal(e.output, 'video');
  assert.equal(e.capabilities.imageToVideo, true);
  assert.deepEqual(e.aspectRatios, ['16:9', '9:16']);
  assert.equal(e.maxRefImages, 1);
  assert.equal(e.refRoles[0].name, 'init');
});

test('deriveModels sorts by id', () => {
  const out = deriveModels([
    { id: 'b/2', provider: 'b', media_type: 'image' },
    { id: 'a/1', provider: 'a', media_type: 'image' },
  ]);
  assert.deepEqual(out.map((m) => m.id), ['a/1', 'b/2']);
});

test('the real models/ directory derives a sane catalog', () => {
  const files = readdirSync(MODELS_DIR).filter((f) => f.endsWith('.yaml') && f !== 'mock.yaml');
  const all = [];
  for (const f of files) {
    const doc = parse(readFileSync(join(MODELS_DIR, f), 'utf8'));
    for (const m of doc?.models ?? []) all.push(m);
  }
  const models = deriveModels(all);
  assert.ok(models.length >= 50, `expected a substantial catalog, got ${models.length}`);

  const dalle3 = models.find((m) => m.id === 'openai/dall-e-3');
  assert.ok(dalle3, 'openai/dall-e-3 should be present');
  assert.deepEqual(dalle3.sizes, ['1024x1024', '1792x1024', '1024x1792']);
  assert.equal(dalle3.capabilities.textToImage, true);
});
