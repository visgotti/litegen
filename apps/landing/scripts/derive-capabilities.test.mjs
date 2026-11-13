import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  capabilitiesFromCatalog,
  deriveCapabilities,
  modelInfoFromCatalog,
  renderGeneratedTs,
} from './derive-capabilities.mjs';
import { MODELS_DIR, readCatalog } from './catalog.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const shipped = () => capabilitiesFromCatalog(readCatalog(MODELS_DIR));

test('text-only model yields text vocab, image output', () => {
  const caps = deriveCapabilities([
    { provider: 'recraft', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: false, max_images: 1 } },
  ]);
  assert.deepEqual(caps.recraft.inputs, { text: true, image: false, multi: false });
  assert.deepEqual(caps.recraft.outputs, { image: true, video: false, model3d: false });
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
  assert.deepEqual(caps.luma.outputs, { image: true, video: true, model3d: false });
});

test('a video-only provider outputs video, not image', () => {
  const caps = deriveCapabilities([
    { provider: 'pixverse', media_type: 'video', capabilities: { supports_text_to_video: true, supports_image_to_video: true, max_images: 1 } },
  ]);
  assert.deepEqual(caps.pixverse.outputs, { image: false, video: true, model3d: false });
});

test('a model3d model sets the model3d output flag and text vocab', () => {
  const caps = deriveCapabilities([
    { provider: 'mock', media_type: 'model3d', capabilities: { supports_text_to_3d: true, supports_image_to_3d: true, max_images: 1 } },
  ]);
  assert.deepEqual(caps.mock.outputs, { image: false, video: false, model3d: true });
  assert.equal(caps.mock.inputs.text, true);
});

test('union across image, video, and model3d models yields all three outputs', () => {
  const caps = deriveCapabilities([
    { provider: 'mock', media_type: 'image', capabilities: { supports_text_to_image: true, max_images: 1 } },
    { provider: 'mock', media_type: 'video', capabilities: { supports_text_to_video: true, max_images: 1 } },
    { provider: 'mock', media_type: 'model3d', capabilities: { supports_text_to_3d: true, max_images: 1 } },
  ]);
  assert.deepEqual(caps.mock.outputs, { image: true, video: true, model3d: true });
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

test('a catalog row reads the way the gateway projects it', () => {
  const info = modelInfoFromCatalog({
    id: 'p/clip',
    provider: 'p',
    media_type: 'video',
    capabilities: { text_to_video: true, image_to_video: true },
    ref_inputs: { max_total: 3, roles: { first_frame: {}, last_frame: {} } },
  });
  assert.equal(info.provider, 'p');
  assert.equal(info.media_type, 'video');
  assert.deepEqual(info.capabilities, {
    supports_text_to_image: false,
    supports_image_to_image: false,
    supports_inpainting: false,
    supports_text_to_video: true,
    supports_image_to_video: true,
    supports_first_frame: true,
    supports_last_frame: true,
    supports_text_to_3d: false,
    supports_image_to_3d: false,
    supports_multiview_to_3d: false,
    supports_background_removal: false,
    supports_segmentation: false,
    supports_source_model: false,
    supports_source_video: false,
    max_images: 3,
  });
});

test('a row without ref_inputs takes one image; a mask role means inpainting', () => {
  assert.equal(modelInfoFromCatalog({ provider: 'p', media_type: 'image', capabilities: {} }).capabilities.max_images, 1);
  const edit = modelInfoFromCatalog({ provider: 'p', media_type: 'image', ref_inputs: { max_total: 2, roles: { init: {}, mask: {} } } });
  assert.equal(edit.capabilities.supports_inpainting, true);
});

test('the catalog derivation leaves the mock provider out', () => {
  const caps = capabilitiesFromCatalog([
    { provider: 'mock', media_type: 'image', capabilities: { text_to_image: true } },
    { provider: 'p', media_type: 'model3d', capabilities: { image_to_3d: true }, ref_inputs: { max_total: 1, roles: { init: {} } } },
  ]);
  assert.deepEqual(Object.keys(caps), ['p']);
  assert.deepEqual(caps.p.outputs, { image: false, video: false, model3d: true });
});

test("the committed snapshot is the shipped catalog's derivation", () => {
  const have = readFileSync(join(ROOT, 'src/components/infra-flow/capabilities.generated.ts'), 'utf8');
  assert.equal(have, renderGeneratedTs(shipped()), 'stale: run `npm run sync-capabilities`');
});

test('the site lists every catalogued provider, and only those', () => {
  const brands = JSON.parse(readFileSync(join(ROOT, 'src/config/providers.json'), 'utf8'));
  assert.deepEqual(brands.map((b) => b.id).sort(), Object.keys(shipped()).sort());
});

test('every diagram node is a catalogued provider', () => {
  const src = readFileSync(join(ROOT, 'src/components/infra-flow/nodes.ts'), 'utf8');
  const ids = [...src.matchAll(/\{ id: '([a-z0-9-]+)', role: 'provider'/g)].map((m) => m[1]);
  assert.ok(ids.length > 0, 'no provider nodes found');
  const caps = shipped();
  for (const id of ids) assert.ok(caps[id], `diagram node ${id} serves no catalog model`);
});

// ─── 3D, background removal, segmentation and sources (D4) ───────────────────

test('a catalog row projects the 3D, background-removal, segmentation and source flags the way GET /v1/models does', () => {
  const caps = (row) => modelInfoFromCatalog({ provider: 'p', capabilities: {}, ...row }).capabilities;
  assert.equal(caps({ media_type: 'model3d', task: 'rig', params: { source_model: { kind: 'source_model', formats: ['glb'] } } }).supports_source_model, true);
  assert.equal(caps({ media_type: 'video', task: 'extend', params: { source_video: { kind: 'source_video', formats: ['mp4'] } } }).supports_source_video, true);
  assert.equal(caps({ media_type: 'model3d', capabilities: { image_to_3d: true } }).supports_image_to_3d, true);
  assert.equal(caps({ media_type: 'model3d', capabilities: { multiview_to_3d: true } }).supports_multiview_to_3d, true);
  assert.equal(caps({ media_type: 'image', capabilities: { background_removal: true } }).supports_background_removal, true);
  assert.equal(
    caps({ media_type: 'image', task: 'segment', params: { prompts: { kind: 'segmentation_prompts', types: ['point'], max_items: 1, max_masks: 1 } } }).supports_segmentation,
    true,
  );
  // By name and kind, as mod.rs reads them: a source param under another name, or
  // segmentation prompts on a model whose task is not segment, project nothing.
  assert.equal(caps({ media_type: 'model3d', params: { mesh: { kind: 'source_model' } } }).supports_source_model, false);
  assert.equal(caps({ media_type: 'model3d', params: { source_model: { kind: 'string' } } }).supports_source_model, false);
  assert.equal(caps({ media_type: 'image', task: 'generate', params: { prompts: { kind: 'segmentation_prompts' } } }).supports_segmentation, false);
});

test('an image-to-3D model, a multiview model, a background remover and a segmenter accept an image', () => {
  for (const flag of ['supports_image_to_3d', 'supports_multiview_to_3d', 'supports_background_removal', 'supports_segmentation']) {
    const caps = deriveCapabilities([{ provider: 'p', media_type: 'model3d', capabilities: { [flag]: true, max_images: 1 } }]);
    assert.deepEqual(caps.p.inputs, { text: false, image: true, multi: false }, flag);
  }
});

test('a provider that only works on a source takes a source, not text (the never-empty rule is for providers with no source)', () => {
  const caps = deriveCapabilities([{ provider: 'm', media_type: 'model3d', capabilities: { supports_source_model: true, max_images: 1 } }]);
  assert.deepEqual(caps.m.inputs, { text: false, image: false, multi: false });
  assert.equal(caps.m.source, true);
  assert.equal(deriveCapabilities([{ provider: 'v', media_type: 'video', capabilities: { supports_source_video: true, max_images: 1 } }]).v.source, true);
  // A provider with no source still never has an empty vocabulary.
  const plain = deriveCapabilities([{ provider: 'w', media_type: 'image', capabilities: { max_images: 1 } }]).w;
  assert.equal(plain.source, false);
  assert.equal(plain.inputs.text, true);
  // Through the shipped catalog: meshy's rows (rig, animate) take only a source_model mesh.
  const meshy = shipped().meshy;
  assert.deepEqual(meshy.inputs, { text: false, image: false, multi: false });
  assert.equal(meshy.source, true);
});
