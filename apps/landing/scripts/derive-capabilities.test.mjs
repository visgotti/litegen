import { test } from 'node:test';
import assert from 'node:assert/strict';
import { deriveCapabilities, FALLBACK_CAPABILITIES } from './derive-capabilities.mjs';

test('text-only model yields text vocab, image output', () => {
  const caps = deriveCapabilities([
    { provider: 'recraft', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: false, max_images: 1 } },
  ]);
  assert.deepEqual(caps.recraft.inputs, { text: true, image: false, multi: false });
  assert.deepEqual(caps.recraft.outputs, { image: true, video: false });
});

test('single-ref model yields text+image', () => {
  const caps = deriveCapabilities([
    { provider: 'bfl', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: true, max_images: 1 } },
  ]);
  assert.deepEqual(caps.bfl.inputs, { text: true, image: true, multi: false });
});

test('multi-image model sets multi and implies image', () => {
  const caps = deriveCapabilities([
    { provider: 'bytedance', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: true, max_images: 6 } },
  ]);
  assert.deepEqual(caps.bytedance.inputs, { text: true, image: true, multi: true });
});

test('union is taken across a provider\'s models; a provider with image + video models outputs both', () => {
  const caps = deriveCapabilities([
    { provider: 'luma', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: true, max_images: 4 } },
    { provider: 'luma', media_type: 'video', capabilities: { supports_text_to_video: true, supports_image_to_video: true, supports_first_frame: true, supports_last_frame: true, max_images: 2 } },
  ]);
  assert.deepEqual(caps.luma.inputs, { text: true, image: true, multi: true });
  assert.deepEqual(caps.luma.outputs, { image: true, video: true });
});

test('a video-only provider outputs video, not image', () => {
  const caps = deriveCapabilities([
    { provider: 'pixverse', media_type: 'video', capabilities: { supports_text_to_video: true, supports_image_to_video: true, max_images: 1 } },
  ]);
  assert.deepEqual(caps.pixverse.outputs, { image: false, video: true });
});

test('first_frame counts as an accepted reference image', () => {
  const caps = deriveCapabilities([
    { provider: 'leonardo', media_type: 'video', capabilities: { supports_image_to_video: true, supports_first_frame: true, max_images: 1 } },
  ]);
  assert.equal(caps.leonardo.inputs.image, true);
});

test('a vocab is never empty (defaults to text)', () => {
  const caps = deriveCapabilities([
    { provider: 'weird', media_type: 'image', capabilities: { max_images: 1 } },
  ]);
  assert.equal(caps.weird.inputs.text, true);
});

test('baked fallback covers all 18 diagram providers', () => {
  const ids = ['openai','stability','replicate','bfl','ideogram','recraft','leonardo','google','fal','runway','luma','kling','minimax','bytedance','bedrock','hunyuan','vidu','pixverse'];
  for (const id of ids) {
    assert.ok(FALLBACK_CAPABILITIES[id], `fallback missing ${id}`);
    const v = FALLBACK_CAPABILITIES[id].inputs;
    assert.ok(v.text || v.image || v.multi, `${id} has empty vocab`);
    const o = FALLBACK_CAPABILITIES[id].outputs;
    assert.ok(o.image || o.video, `${id} has no output modality`);
  }
});
