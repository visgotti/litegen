import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { parse } from 'yaml';
import { deriveModel, deriveModels, buildProviderModels } from './derive-models.mjs';

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

test('deriveModel maps 3D capability flags', () => {
  const e = deriveModel({
    id: 'mock/mesh-3d',
    provider: 'mock',
    media_type: 'model3d',
    display_name: 'Mock Mesh 3D',
    capabilities: { text_to_3d: true, image_to_3d: true, multiview_to_3d: false },
    prompt: { required: true, max_length: 4000 },
    params: {
      output_format: { kind: 'string', enum_values: ['glb'], default: 'glb' },
      target_polycount: { kind: 'int', min: 100, max: 300000 },
    },
    ref_inputs: { max_total: 1, roles: { init: { required: false, min_count: 0, max_count: 1 } } },
    tags: ['mock', 'test'],
  });

  assert.equal(e.mediaType, 'model3d');
  assert.equal(e.output, 'model3d');
  assert.equal(e.capabilities.textTo3d, true);
  assert.equal(e.capabilities.imageTo3d, true);
  assert.equal(e.capabilities.multiviewTo3d, false);
  assert.equal(e.capabilities.textToImage, false);
});

test('deriveModels sorts by id', () => {
  const out = deriveModels([
    { id: 'b/2', provider: 'b', media_type: 'image' },
    { id: 'a/1', provider: 'a', media_type: 'image' },
  ]);
  assert.deepEqual(out.map((m) => m.id), ['a/1', 'b/2']);
});

test('buildProviderModels buckets model3d output separately from image/video', () => {
  const models = deriveModels([
    { id: 'mock/mesh-3d', provider: 'mock', media_type: 'model3d', display_name: 'Mock Mesh 3D' },
    { id: 'mock/image-gen', provider: 'mock', media_type: 'image', display_name: 'Mock Image' },
    { id: 'mock/video-gen', provider: 'mock', media_type: 'video', display_name: 'Mock Video' },
  ]);
  const out = buildProviderModels(models);
  assert.deepEqual(out.mock, {
    image: ['Mock Image'],
    video: ['Mock Video'],
    model3d: ['Mock Mesh 3D'],
  });
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

  // openai/dall-e-3 was retired by OpenAI on 2026-05-12; gpt-image-1 is the
  // replacement and carries the three standard GPT-image sizes.
  const gptImage1 = models.find((m) => m.id === 'openai/gpt-image-1');
  assert.ok(gptImage1, 'openai/gpt-image-1 should be present');
  assert.deepEqual(gptImage1.sizes, ['1024x1024', '1536x1024', '1024x1536']);
  assert.equal(gptImage1.capabilities.textToImage, true);
});
