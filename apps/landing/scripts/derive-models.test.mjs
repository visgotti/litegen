import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { parse } from 'yaml';
import { deriveModel, deriveModels, buildProviderModels, PROVIDER_DOCS } from './derive-models.mjs';
import { readCatalog } from './catalog.mjs';

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

test('a model links its own docs_url, else its provider docs', () => {
  const own = deriveModel({
    id: 'fal/trellis', provider: 'fal', media_type: 'model3d',
    docs_url: 'https://fal.ai/models/fal-ai/trellis/api',
  });
  assert.equal(own.referenceUrl, 'https://fal.ai/models/fal-ai/trellis/api');
  const fallback = deriveModel({ id: 'fal/flux/schnell', provider: 'fal', media_type: 'image' });
  assert.equal(fallback.referenceUrl, PROVIDER_DOCS.fal);
  // Only https is trusted, mirroring the catalog loader's rule.
  const insecure = deriveModel({ id: 'x/y', provider: 'fal', media_type: 'image', docs_url: 'http://example.com' });
  assert.equal(insecure.referenceUrl, PROVIDER_DOCS.fal);
});

test('a model that takes no prompt keeps maxLength 0', () => {
  const m = deriveModel({ id: 'fal/trellis', provider: 'fal', media_type: 'model3d', prompt: { required: false, max_length: 0 } });
  assert.deepEqual(m.promptLimits, { required: false, minLength: undefined, maxLength: 0 });
});

test('a 3D format set is the vendor\'s own list, with its array default intact', () => {
  // The gateway adds the containers it converts to; the page states that
  // rather than mirroring a rule that grows with litegen's mesh support.
  const formats = deriveModel({
    id: 'fal/trellis', provider: 'fal', media_type: 'model3d',
    params: { output_formats: { kind: 'string_array', enum_values: ['glb'], max_items: 1, default: ['glb'] } },
  }).params.find((p) => p.name === 'output_formats');
  assert.deepEqual(formats.enum, ['glb']);
  assert.deepEqual(formats.default, ['glb']);
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

  // openai/dall-e-3 was retired by OpenAI on 2026-05-12 and gpt-image-1 on
  // 2026-10-23; GPT Image 2.5 Sunburst is the replacement OpenAI recommends.
  const sunburst = models.find((m) => m.id === 'openai/gpt-image-2.5-sunburst');
  assert.ok(sunburst, 'openai/gpt-image-2.5-sunburst should be present');
  assert.equal(sunburst.capabilities.textToImage, true);
  assert.equal(models.find((m) => m.id === 'openai/gpt-image-1'), undefined, 'gpt-image-1 is shut down');
});

test("deriveModel carries the model's task, generate when the row names none", () => {
  const base = { provider: 'mock', media_type: 'model3d', prompt: {}, params: {} };
  assert.equal(deriveModel({ ...base, id: 'mock/rig-3d', task: 'rig' }).task, 'rig');
  assert.equal(deriveModel({ ...base, id: 'mock/mesh-3d' }).task, 'generate');
});

// ─── Prices (D1): the exact amount, its unit, and its rules in order ─────────

test('a price carries its unit and its rules, in order', () => {
  // bedrock/luma.ray-v2:0, cut down to what its price needs.
  const e = deriveModel({
    id: 'bedrock/luma.ray-v2:0', provider: 'bedrock', media_type: 'video',
    pricing: { base_cost_usd: 7.5, variable_pricing: [{ per_second_usd: 1.5 }, { when: { resolution: '540p' }, per_second_usd: 0.75 }] },
  });
  assert.deepEqual(e.pricing, {
    baseCostUsd: 7.5,
    unit: 'video',
    rules: [
      { when: [], effect: { kind: 'perSecond', value: 1.5 } },
      { when: [{ fact: 'resolution', is: 'eq', value: '540p' }], effect: { kind: 'perSecond', value: 0.75 } },
    ],
  });
});

test('a price names what one charge buys', () => {
  const unit = (row) => deriveModel({ id: 'x/y', provider: 'x', pricing: { base_cost_usd: 1 }, ...row }).pricing.unit;
  assert.equal(unit({ media_type: 'image' }), 'image');
  // One billed vendor call: a multiview set (router.rs one_call) and a segmentation (row_price_usd).
  assert.equal(unit({ media_type: 'image', task: 'multiview' }), 'request');
  assert.equal(unit({ media_type: 'image', task: 'segment' }), 'request');
  assert.equal(unit({ media_type: 'video' }), 'video');
  assert.equal(unit({ media_type: 'model3d', task: 'rig' }), 'model3d');
});

test('every rule shape the catalog uses keeps its condition and its one effect', () => {
  const cases = [
    [{ when: { references: '*' }, usd: 0.3 },
      { when: [{ fact: 'references', is: 'sent' }], effect: { kind: 'usd', value: 0.3 } }],
    [{ when: { size: '*', megapixels: { gt: 1.048576, lte: 4.194304 } }, usd: 0.7 },
      { when: [{ fact: 'size', is: 'sent' }, { fact: 'megapixels', is: 'range', gt: 1.048576, lte: 4.194304 }], effect: { kind: 'usd', value: 0.7 } }],
    [{ when: { audio: true }, add_usd: 0.1 },
      { when: [{ fact: 'audio', is: 'eq', value: true }], effect: { kind: 'addUsd', value: 0.1 } }],
    [{ when: { 'references.reference': '*' }, usd: 0.4 },
      { when: [{ fact: 'references.reference', is: 'sent' }], effect: { kind: 'usd', value: 0.4 } }],
    [{ when: { 'extra.addons': 'HighPack' }, multiply: 2 },
      { when: [{ fact: 'extra.addons', is: 'eq', value: 'HighPack' }], effect: { kind: 'multiply', value: 2 } }],
    [{ per_megapixel_usd: 0.04 },
      { when: [], effect: { kind: 'perMegapixel', value: 0.04 } }],
    [{ multiply_per_megapixel_above: 1.048576 },
      { when: [], effect: { kind: 'multiplyPerMegapixelAbove', value: 1.048576 } }],
    [{ per_item_usd: 0.06, per_item: 'animations' },
      { when: [], effect: { kind: 'perItem', value: 0.06, list: 'animations' } }],
    [{ when: { duration_seconds: 5 }, usd: 0.1 },
      { when: [{ fact: 'duration_seconds', is: 'eq', value: 5 }], effect: { kind: 'usd', value: 0.1 } }],
    [{ when: { references: { gte: 1 } }, per_megapixel_usd: 0.01 },
      { when: [{ fact: 'references', is: 'range', gte: 1 }], effect: { kind: 'perMegapixel', value: 0.01 } }],
    [{ add_usd: -0.01 },
      { when: [], effect: { kind: 'addUsd', value: -0.01 } }],
  ];
  for (const [rule, want] of cases) {
    const e = deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', pricing: { base_cost_usd: 1, variable_pricing: [rule] } });
    assert.deepEqual(e.pricing.rules, [want], JSON.stringify(rule));
  }
});

test('a price rule the page cannot word stops the derivation', () => {
  const row = (rule) => ({ id: 'x/y', provider: 'x', media_type: 'image', pricing: { base_cost_usd: 1, variable_pricing: [rule] } });
  // An effect pricing.rs does not have (a future one must reach the page, not vanish).
  assert.throws(() => deriveModel(row({ per_frame_usd: 0.1 })), /x\/y.*effect/);
  // Zero effects, two effects, and a per-item rate with no list.
  assert.throws(() => deriveModel(row({ when: { audio: true } })), /x\/y.*effect/);
  assert.throws(() => deriveModel(row({ usd: 0.1, add_usd: 0.2 })), /x\/y.*effect/);
  assert.throws(() => deriveModel(row({ per_item_usd: 0.06 })), /x\/y.*effect/);
  // A range bound pricing.rs does not know, and a condition value it cannot match.
  assert.throws(() => deriveModel(row({ when: { megapixels: { above: 1 } }, usd: 0.1 })), /x\/y.*megapixels/);
  assert.throws(() => deriveModel(row({ when: { quality: ['high', 'hd'] }, usd: 0.1 })), /x\/y.*quality/);
});

test('a "sent" condition never names a fact every request has', () => {
  // pricing.rs matches "*" on any present fact: duration_seconds and megapixels
  // always are, and so is a param with a default. Such a rule always applies,
  // so "X sent" would mislead; the derivation refuses it.
  const row = (when, params = {}) => ({
    id: 'x/y', provider: 'x', media_type: 'video', params,
    pricing: { base_cost_usd: 1, variable_pricing: [{ when, usd: 0.1 }] },
  });
  assert.throws(() => deriveModel(row({ duration_seconds: '*' })), /x\/y.*duration_seconds/);
  assert.throws(() => deriveModel(row({ megapixels: '*' })), /x\/y.*megapixels/);
  assert.throws(() => deriveModel(row({ resolution: '*' }, { resolution: { kind: 'string', enum_values: ['720p'], default: '720p' } })), /x\/y.*resolution/);
  // A param with no default, or a size (never a default fact), is fine.
  assert.equal(deriveModel(row({ resolution: '*' }, { resolution: { kind: 'string', enum_values: ['720p'] } })).pricing.rules[0].when[0].is, 'sent');
  assert.equal(deriveModel(row({ size: '*' }, { size: { kind: 'size', mode: 'enum', values: [[1024, 1024]] } })).pricing.rules[0].when[0].is, 'sent');
});

test('a condition on a default the engine does not apply stops the derivation', () => {
  // pricing.rs `param_default` gives no fact for an aspect_ratio default, so a
  // rule naming the default only applies when the request sends it: the page's
  // "as sent, else its default" would mislead, so the derivation refuses it.
  const row = (want) => ({
    id: 'x/y', provider: 'x', media_type: 'video',
    params: { aspect_ratio: { kind: 'aspect_ratio', allowed: ['16:9', '9:16'], default: '16:9' } },
    pricing: { base_cost_usd: 1, variable_pricing: [{ when: { aspect_ratio: want }, usd: 0.1 }] },
  });
  assert.throws(() => deriveModel(row('16:9')), /x\/y.*aspect_ratio/);
  // Any other value reads the same either way.
  assert.deepEqual(deriveModel(row('9:16')).pricing.rules[0].when, [{ fact: 'aspect_ratio', is: 'eq', value: '9:16' }]);
});

test('a row with a directions param has no single unit, so the derivation stops', () => {
  // router.rs bills a request that sends `directions` as one call, and one
  // without it per image: the page would need a second unit to say so.
  assert.throws(
    () => deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', params: { directions: { kind: 'int', values: [4, 8] } }, pricing: { base_cost_usd: 1 } }),
    /x\/y.*directions/,
  );
});

test('every shipped price rule reaches the page', () => {
  const missing = readCatalog()
    .filter((row) => (deriveModel(row).pricing?.rules?.length ?? 0) !== (row.pricing?.variable_pricing ?? []).length)
    .map((row) => row.id);
  assert.deepEqual(missing, []);
});

// ─── What a model takes (D3): lists, lifecycle, freeform sizes, words, limits ─

test('a number param that takes a list keeps the list', () => {
  const e = deriveModel({
    id: 'bedrock/luma.ray-v2:0', provider: 'bedrock', media_type: 'video',
    params: { duration_seconds: { kind: 'float', min: 5, max: 9, default: 5, values: [5, 9] } },
  });
  assert.deepEqual(e.params[0].values, [5, 9]);
  const fps = deriveModel({ id: 'x/y', provider: 'x', media_type: 'video', params: { fps: { kind: 'int', values: [24, 25, 48, 50] } } }).params[0];
  assert.deepEqual(fps.values, [24, 25, 48, 50]);
  const range = deriveModel({ id: 'x/y', provider: 'x', media_type: 'video', params: { fps: { kind: 'int', min: 1, max: 60 } } }).params[0];
  assert.equal('values' in range, false);
});

test('a model carries its lifecycle, word for word', () => {
  const row = (more) => ({ id: 'google/x', provider: 'google', media_type: 'image', ...more });
  assert.deepEqual(
    deriveModel(row({ deprecated_at: '2026-10-06', replaced_by: 'google/gemini-nano-banana-2.1' })).lifecycle,
    { deprecatedAt: '2026-10-06', replacedBy: 'google/gemini-nano-banana-2.1' },
  );
  assert.deepEqual(
    deriveModel(row({ retires_at: '2027-03-15', replaced_by: 'google/gemini-3.1-flash-lite-image' })).lifecycle,
    { retiresAt: '2027-03-15', replacedBy: 'google/gemini-3.1-flash-lite-image' },
  );
  const active = deriveModel(row({}));
  assert.equal('lifecycle' in active, false);
});

test('a freeform size carries its bounds', () => {
  const sized = (size) => deriveModel({ id: 'bfl/x', provider: 'bfl', media_type: 'image', params: { size } });
  // bfl/flux-pro-1.1
  assert.deepEqual(
    sized({ kind: 'size', mode: 'freeform', min_width: 256, max_width: 1440, min_height: 256, max_height: 1440, multiple_of: 32 }).sizeRange,
    { minWidth: 256, maxWidth: 1440, minHeight: 256, maxHeight: 1440, multipleOf: 32 },
  );
  // bfl/flux-3-image
  assert.deepEqual(
    sized({ kind: 'size', mode: 'freeform', min_width: 256, max_width: 6256, min_height: 256, max_height: 6256, max_pixels: 16822272, max_aspect: 8 }).sizeRange,
    { minWidth: 256, maxWidth: 6256, minHeight: 256, maxHeight: 6256, maxPixels: 16822272, maxAspect: 8 },
  );
  // openai/gpt-image-2's size has every bound
  assert.deepEqual(
    sized({ kind: 'size', mode: 'freeform', min_width: 16, max_width: 3840, min_height: 16, max_height: 3840, multiple_of: 16, min_pixels: 655360, max_pixels: 8294400, max_aspect: 3 }).sizeRange,
    { minWidth: 16, maxWidth: 3840, minHeight: 16, maxHeight: 3840, multipleOf: 16, minPixels: 655360, maxPixels: 8294400, maxAspect: 3 },
  );
  const listed = sized({ kind: 'size', mode: 'enum', values: [[1024, 1024]] });
  assert.equal('sizeRange' in listed, false);
  assert.deepEqual(listed.sizes, ['1024x1024']);
});

test('a param keeps its words and its limits', () => {
  const params = deriveModel({
    id: 'x/y', provider: 'x', media_type: 'image',
    params: {
      search_prompt: { kind: 'string', max_length: 10000, label: 'Find', description: 'What to replace' },
      layout: { kind: 'layout', kinds: ['object', 'text'], max_items: 32 },
      output_formats: { kind: 'string_array', enum_values: ['glb'], max_items: 1, default: ['glb'] },
    },
  }).params;
  const find = params.find((p) => p.name === 'search_prompt');
  assert.equal(find.maxLength, 10000);
  assert.equal(find.label, 'Find');
  assert.equal(find.description, 'What to replace');
  assert.equal(params.find((p) => p.name === 'layout').maxItems, 32);
  // For output_formats, max_items counts vendor jobs, not members (schema.rs), so it is not "up to N".
  assert.equal('maxItems' in params.find((p) => p.name === 'output_formats'), false);
  // A param with no words gets no keys for them.
  assert.equal('label' in params.find((p) => p.name === 'layout'), false);
});

test('the size and aspect ratio keep their words', () => {
  // recraft/recraftv3 describes its aspect_ratio; size and aspect_ratio are shown on their own lines.
  const e = deriveModel({
    id: 'recraft/recraftv3', provider: 'recraft', media_type: 'image',
    params: {
      size: { kind: 'size', mode: 'enum', values: [[1024, 1024]], label: 'Size', description: 'Exact pixels' },
      aspect_ratio: { kind: 'aspect_ratio', allowed: ['1:1'], label: 'Aspect ratio', description: "Sent as Recraft's size in w:h form; an exact size wins." },
    },
  });
  assert.equal(e.sizeLabel, 'Size');
  assert.equal(e.sizeDescription, 'Exact pixels');
  assert.equal(e.aspectRatioLabel, 'Aspect ratio');
  assert.equal(e.aspectRatioDescription, "Sent as Recraft's size in w:h form; an exact size wins.");
  const plain = deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', params: { aspect_ratio: { kind: 'aspect_ratio', allowed: ['1:1'] } } });
  assert.equal('aspectRatioDescription' in plain, false);
});

// validator.rs `check_frame_shape` (E5): on a row whose aspect ratio has
// `frames_set_shape`, an aspect_ratio sent with a first_frame, last_frame or
// init image is refused (strict) or dropped and named (lax). A role-less image
// has taken the row's default_role by then. On a `without_references` row a
// `reference` image beside the frame lifts the rule; on an `always` row nothing does.
test('a ratio a frame image overrides says which images set the shape', () => {
  const video = (shape, roles, defaultRole = 'first_frame') => deriveModel({
    id: 'x/y', provider: 'x', media_type: 'video',
    params: { aspect_ratio: { kind: 'aspect_ratio', allowed: ['16:9', '9:16', '1:1'], ...(shape ? { frames_set_shape: shape } : {}) } },
    ref_inputs: { max_total: roles.length, default_role: defaultRole, roles: Object.fromEntries(roles.map((r) => [r, { max_count: 1 }])) },
  });
  // kling/video-kling-v3: no reference role, so nothing lifts the rule
  assert.deepEqual(video('without_references', ['first_frame', 'last_frame']).aspectRatioFrames, {
    roles: ['first_frame', 'last_frame'], unlessReference: false, defaultRole: 'first_frame',
  });
  // vidu/viduq1: a reference image beside the frame lifts it
  assert.deepEqual(video('without_references', ['first_frame', 'last_frame', 'reference']).aspectRatioFrames, {
    roles: ['first_frame', 'last_frame'], unlessReference: true, defaultRole: 'first_frame',
  });
  // runway/kling_4_flash: always, a reference image included
  assert.deepEqual(video('always', ['first_frame', 'reference']).aspectRatioFrames, {
    roles: ['first_frame'], unlessReference: false, defaultRole: 'first_frame',
  });
  // A role-less image that lands on a non-frame role does not set the shape.
  assert.deepEqual(video('without_references', ['first_frame', 'reference'], 'reference').aspectRatioFrames, {
    roles: ['first_frame'], unlessReference: true,
  });
  // No rule, or no frame role that could trigger it: nothing to say.
  assert.equal('aspectRatioFrames' in video(undefined, ['first_frame']), false);
  assert.equal('aspectRatioFrames' in video('always', ['reference'], 'reference'), false);
});

test('a frame rule the page cannot word stops the derivation', () => {
  assert.throws(
    () => deriveModel({
      id: 'x/y', provider: 'x', media_type: 'video',
      params: { aspect_ratio: { kind: 'aspect_ratio', allowed: ['1:1'], frames_set_shape: 'sometimes' } },
      ref_inputs: { max_total: 1, default_role: 'first_frame', roles: { first_frame: { max_count: 1 } } },
    }),
    /x\/y.*frames_set_shape/,
  );
});

test('every ratio a frame image sets in the catalog reaches the page', () => {
  const missing = readCatalog()
    .filter((row) => row.params?.aspect_ratio?.frames_set_shape !== undefined)
    .filter((row) => Object.keys(row.ref_inputs?.roles ?? {}).some((r) => ['first_frame', 'last_frame', 'init'].includes(r)))
    .filter((row) => deriveModel(row).aspectRatioFrames === undefined)
    .map((row) => row.id);
  assert.deepEqual(missing, []);
});

test('background removal and native pixel art reach the page', () => {
  assert.equal(deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', capabilities: { background_removal: true } }).capabilities.backgroundRemoval, true);
  assert.equal(deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', capabilities: { native_pixel_art: true } }).capabilities.nativePixelArt, true);
  const plain = deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', capabilities: { text_to_image: true } }).capabilities;
  assert.equal(plain.backgroundRemoval, false);
  assert.equal(plain.nativePixelArt, false);
});

test("an operation's source mesh and clips keep their contract", () => {
  const params = (p) => deriveModel({ id: 'x/y', provider: 'x', media_type: 'model3d', task: 'rig', params: p }).params;
  // tripo/rig
  const tripo = params({ source_model: { kind: 'source_model', formats: ['glb', 'fbx', 'obj', 'stl'], max_bytes: 150000000, label: 'Source mesh' } })[0];
  assert.deepEqual(tripo.formats, ['glb', 'fbx', 'obj', 'stl']);
  assert.equal(tripo.maxBytes, 150000000);
  assert.equal('maxFaces' in tripo, false);
  assert.equal('requiresSkin' in tripo, false);
  // fal/meshy-rig
  const meshy = params({ source_model: { kind: 'source_model', formats: ['glb'], max_bytes: 100000000, max_faces: 320000, requires_texture: true } })[0];
  assert.equal(meshy.maxFaces, 320000);
  assert.equal(meshy.requiresTexture, true);
  // meshy/animate: the clips it takes, by the vendor's own id
  const animate = params({
    source_model: { kind: 'source_model', formats: ['glb'], requires_skin: true },
    animations: { kind: 'animation_list', max_items: 10, presets: [{ id: '0', label: 'Idle', canonical: 'idle' }, { id: '1', label: 'Walking Woman' }] },
  });
  assert.equal(animate[0].requiresSkin, true);
  assert.equal(animate[1].maxItems, 10);
  assert.deepEqual(animate[1].presets, [{ id: '0', label: 'Idle', canonical: 'idle' }, { id: '1', label: 'Walking Woman' }]);
});

test('a palette, a segmentation, an expand and a layout keep their limits', () => {
  const params = deriveModel({
    id: 'x/y', provider: 'x', media_type: 'image',
    params: {
      palette: { kind: 'palette', forced: true, max_colors: 10 },
      prompts: { kind: 'segmentation_prompts', types: ['point', 'box', 'text'], max_items: 64, max_masks: 32 },
      expand: { kind: 'expand', max_px: 2048 },
      layout: { kind: 'layout', kinds: ['object', 'text'], max_items: 32 },
    },
  }).params;
  const by = (n) => params.find((p) => p.name === n);
  assert.equal(by('palette').maxColors, 10);
  assert.equal(by('palette').forced, true);
  assert.deepEqual(by('prompts').enum, ['point', 'box', 'text']);
  assert.equal(by('prompts').maxItems, 64);
  assert.equal(by('prompts').maxMasks, 32);
  assert.equal(by('expand').maxPx, 2048);
  assert.deepEqual(by('layout').enum, ['object', 'text']);
  const hint = deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', params: { palette: { kind: 'palette', forced: false, max_colors: 5 } } }).params[0];
  assert.equal('forced' in hint, false);
});

test('every values list in the catalog reaches the page', () => {
  const missing = [];
  for (const row of readCatalog()) {
    const derived = deriveModel(row);
    for (const [name, p] of Object.entries(row.params ?? {})) {
      if ((p.kind === 'int' || p.kind === 'float') && Array.isArray(p.values) && p.values.length > 0) {
        const got = derived.params.find((d) => d.name === name)?.values;
        if (JSON.stringify(got) !== JSON.stringify(p.values)) missing.push(`${row.id}.${name}`);
      }
    }
  }
  assert.deepEqual(missing, []);
});

test('every source contract, clip list and lifecycle in the catalog reaches the page', () => {
  const missing = [];
  for (const row of readCatalog()) {
    const derived = deriveModel(row);
    for (const [name, p] of Object.entries(row.params ?? {})) {
      const d = derived.params.find((x) => x.name === name);
      if (p.kind === 'source_model') {
        const want = [p.formats, p.max_bytes, p.max_faces, p.requires_skin === true, p.requires_texture === true];
        const got = [d?.formats, d?.maxBytes, d?.maxFaces, d?.requiresSkin === true, d?.requiresTexture === true];
        if (JSON.stringify(got) !== JSON.stringify(want)) missing.push(`${row.id}.${name}`);
      }
      if (p.kind === 'animation_list' && JSON.stringify(d?.presets?.map((x) => x.id)) !== JSON.stringify(p.presets.map((x) => x.id))) {
        missing.push(`${row.id}.${name}`);
      }
    }
    const life = [row.deprecated_at, row.retires_at, row.replaced_by];
    const got = [derived.lifecycle?.deprecatedAt, derived.lifecycle?.retiresAt, derived.lifecycle?.replacedBy];
    if (JSON.stringify(got) !== JSON.stringify(life)) missing.push(`${row.id} lifecycle`);
  }
  assert.deepEqual(missing, []);
});

// ─── What a model does (D4): the badges, decided at build time ───────────────

test('the image and video badges', () => {
  const dalle = deriveModel({ id: 'openai/dall-e-3', provider: 'openai', media_type: 'image', capabilities: { text_to_image: true } });
  assert.deepEqual(dalle.modalities, ['textToImage']);
  const sora = deriveModel({
    id: 'openai/sora', provider: 'openai', media_type: 'video',
    capabilities: { text_to_video: true, image_to_video: true },
    ref_inputs: { max_total: 1, roles: { init: { required: false, min_count: 0, max_count: 1 } } },
  });
  assert.deepEqual(sora.modalities, ['textToVideo', 'imageToVideo']);
  const edit = deriveModel({
    id: 'x/edit', provider: 'x', media_type: 'image',
    capabilities: { text_to_image: true, image_to_image: true, inpainting: true },
    ref_inputs: { max_total: 2, roles: { init: {}, mask: {} } },
  });
  assert.deepEqual(edit.modalities, ['textToImage', 'imageToImage', 'inpainting']);
});

test('a 3D generator, a background remover and a pixel-art model say what they do', () => {
  const modalities = (capabilities) => deriveModel({ id: 'x/y', provider: 'x', media_type: 'model3d', capabilities }).modalities;
  // fal/hyper3d-rodin
  assert.deepEqual(modalities({ text_to_3d: true, image_to_3d: true, multiview_to_3d: true }), ['textTo3d', 'imageTo3d', 'multiviewTo3d']);
  // fal/trellis
  assert.deepEqual(modalities({ image_to_3d: true }), ['imageTo3d']);
  // fal/bria-rmbg-2
  assert.deepEqual(modalities({ background_removal: true }), ['backgroundRemoval']);
  // pixellab/pixflux
  assert.deepEqual(modalities({ text_to_image: true, native_pixel_art: true }), ['textToImage', 'pixelArt']);
});

test('the inpainting badge reads the mask role, as GET /v1/models does', () => {
  // project_model_info: supports_inpainting is "takes a mask", read the way the validator reads it.
  const badges = (capabilities, roles) => deriveModel({ id: 'x/y', provider: 'x', media_type: 'image', capabilities, ref_inputs: { max_total: 2, roles } }).modalities;
  assert.deepEqual(badges({ image_to_image: true }, { init: {}, mask: {} }), ['imageToImage', 'inpainting']);
  assert.deepEqual(badges({ image_to_image: true, inpainting: true }, { init: {} }), ['imageToImage']);
});

test('every shipped model shows what it does', () => {
  const silent = readCatalog()
    .map(deriveModel)
    .filter((m) => m.modalities.length === 0 && m.task === 'generate')
    .map((m) => m.id);
  assert.deepEqual(silent, []);
});

test('a model that takes no prompt says what it works on', () => {
  const on = (row) => deriveModel({ id: 'x/y', provider: 'x', prompt: { required: false, max_length: 0 }, ...row }).worksOn;
  assert.equal(on({ media_type: 'model3d', task: 'rig', params: { source_model: { kind: 'source_model', formats: ['glb'] } } }), 'sourceModel');
  assert.equal(on({ media_type: 'video', task: 'extend', params: { source_video: { kind: 'source_video', formats: ['mp4'] } } }), 'sourceVideo');
  assert.equal(on({ media_type: 'image', task: 'segment', params: { prompts: { kind: 'segmentation_prompts', types: ['point'], max_items: 1, max_masks: 1 } } }), 'segmentation');
  // Read by name and kind, as the API's supports_source_model and supports_segmentation are.
  assert.equal(on({ media_type: 'model3d', params: { mesh: { kind: 'source_model' } } }), undefined);
  assert.equal(on({ media_type: 'image', task: 'generate', params: { prompts: { kind: 'segmentation_prompts' } } }), undefined);
  assert.equal(on({ media_type: 'model3d', capabilities: { image_to_3d: true } }), undefined);
  // The shipped rows: the five mesh operations and the two segmenters.
  const shipped = Object.fromEntries(readCatalog().map(deriveModel).filter((m) => m.worksOn).map((m) => [m.id, m.worksOn]));
  assert.deepEqual(shipped, {
    'fal/meshy-rig': 'sourceModel',
    'fal/sam-3': 'segmentation',
    'fal/sam-3.1': 'segmentation',
    'meshy/animate': 'sourceModel',
    'meshy/rig': 'sourceModel',
    'tripo/animate': 'sourceModel',
    'tripo/rig': 'sourceModel',
  });
});
