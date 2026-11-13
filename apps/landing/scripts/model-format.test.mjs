import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTranslator } from 'next-intl';
import * as fmt from '../src/lib/model-format.mjs';
import { readCatalog } from './catalog.mjs';
import { deriveModel } from './derive-models.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

/** The page's own translator for the `reference` messages, collecting next-intl's errors. */
function translator(locale, errors = []) {
  const messages = JSON.parse(readFileSync(join(ROOT, 'messages', `${locale}.json`), 'utf8'));
  return createTranslator({ locale, messages, namespace: 'reference', onError: (e) => errors.push(`${e.code}: ${e.message}`) });
}

test('a freeform size reads as its bounds', () => {
  const en = translator('en');
  // bfl/flux-pro-1.1
  assert.equal(
    fmt.sizeRangeText({ minWidth: 256, maxWidth: 1440, minHeight: 256, maxHeight: 1440, multipleOf: 32 }, en, 'en'),
    '256–1440 × 256–1440 px · multiples of 32',
  );
  // openai/gpt-image-2
  assert.equal(
    fmt.sizeRangeText({ minWidth: 16, maxWidth: 3840, minHeight: 16, maxHeight: 3840, multipleOf: 16, minPixels: 655360, maxPixels: 8294400, maxAspect: 3 }, en, 'en'),
    '16–3840 × 16–3840 px · multiples of 16 · at least 655,360 pixels · at most 8,294,400 pixels · sides up to 3:1',
  );
  // bfl/flux-3-image, in es
  assert.equal(
    fmt.sizeRangeText({ minWidth: 256, maxWidth: 6256, minHeight: 256, maxHeight: 6256, maxPixels: 16822272, maxAspect: 8 }, translator('es'), 'es'),
    '256–6256 × 256–6256 px · como máximo 16.822.272 píxeles · lados hasta 8:1',
  );
  // A fixed side is one number.
  assert.equal(fmt.sizeRangeText({ minWidth: 1024, maxWidth: 1024, minHeight: 512, maxHeight: 2048 }, en, 'en'), '1024 × 512–2048 px');
});

test('a lifecycle reads word for word', () => {
  const en = translator('en');
  assert.equal(
    fmt.lifecycleText({ deprecatedAt: '2026-10-06', replacedBy: 'google/gemini-nano-banana-2.1' }, en),
    'deprecated by the vendor on 2026-10-06 · use google/gemini-nano-banana-2.1 instead',
  );
  assert.equal(
    fmt.lifecycleText({ retiresAt: '2027-03-15', replacedBy: 'google/gemini-3.1-flash-lite-image' }, en),
    'served through 2027-03-15, then retired · use google/gemini-3.1-flash-lite-image instead',
  );
  assert.equal(
    fmt.lifecycleText({ deprecatedAt: '2026-10-06', replacedBy: 'google/gemini-nano-banana-2.1' }, translator('es')),
    'obsoleto para el proveedor desde el 2026-10-06 · usa google/gemini-nano-banana-2.1 en su lugar',
  );
});

// A caller may send a source in any of the 11 containers litegen reads
// (validator.rs, Model3dFormat::ALL); `prepare` (proxy/model3d_source.rs)
// converts one the vendor does not take to the first container in `formats`
// it can write. So `formats` is what the vendor takes, never all a caller may send.
test('a source mesh reads as the containers its vendor takes, and litegen converts any other mesh it reads', () => {
  const en = translator('en');
  // tripo/rig's source mesh
  assert.deepEqual(
    fmt.paramLimitsText({ name: 'source_model', kind: 'source_model', formats: ['glb', 'fbx', 'obj', 'stl'], maxBytes: 150000000 }, en, 'en'),
    ['the vendor takes glb, fbx, obj, stl; litegen converts any other mesh it reads', 'up to 150 MB'],
  );
  // fal/meshy-rig's
  assert.deepEqual(
    fmt.paramLimitsText({ name: 'source_model', kind: 'source_model', formats: ['glb'], maxBytes: 100000000, maxFaces: 320000, requiresTexture: true }, en, 'en'),
    ['the vendor takes glb; litegen converts any other mesh it reads', 'up to 100 MB', 'at most 320,000 triangles', 'must be textured'],
  );
  // meshy/animate's, in es
  assert.deepEqual(
    fmt.paramLimitsText({ name: 'source_model', kind: 'source_model', formats: ['glb'], requiresSkin: true }, translator('es'), 'es'),
    ['el proveedor recibe glb; litegen convierte cualquier otra malla que lea', 'debe tener esqueleto'],
  );
});

// `max_faces` is "Most triangles the vendor accepts" (schema.rs): `prepare`
// (proxy/model3d_source.rs) compares the mesh's `triangle_count()` with it and
// refuses with "the source has N triangles". A quad counts as two, so a
// 200,000-quad mesh is refused under a 320,000 limit.
test("a source mesh's face limit reads as the triangles litegen counts", () => {
  // meshy/rig's source mesh, in es and en
  const mesh = { name: 'source_model', kind: 'source_model', formats: ['glb'], maxFaces: 320000 };
  assert.ok(fmt.paramLimitsText(mesh, translator('es'), 'es').includes('como máximo 320.000 triángulos'), fmt.paramLimitsText(mesh, translator('es'), 'es').join(' · '));
  assert.ok(fmt.paramLimitsText(mesh, translator('en'), 'en').includes('at most 320,000 triangles'), fmt.paramLimitsText(mesh, translator('en'), 'en').join(' · '));
});

test("a param's limits read as the validator enforces them", () => {
  const en = translator('en');
  assert.deepEqual(fmt.paramLimitsText({ name: 'search_prompt', kind: 'string', maxLength: 10000 }, en, 'en'), ['up to 10000 chars']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'animations', kind: 'animation_list', maxItems: 10, presets: [] }, en, 'en'), ['up to 10']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'palette', kind: 'palette', maxColors: 10, forced: true }, en, 'en'), ['up to 10 colors', 'every pixel is one of them']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'prompts', kind: 'segmentation_prompts', maxItems: 64, maxMasks: 32 }, en, 'en'), ['up to 64', 'up to 32 masks per answer']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'expand', kind: 'expand', maxPx: 2048 }, en, 'en'), ['up to 2,048 px per side']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'layout', kind: 'layout', maxItems: 32, singleWordText: true }, en, 'en'), ['up to 32', 'one word per text box']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'seed', kind: 'seed' }, en, 'en'), []);
});

// validator.rs `check_frame_shape` refuses (strict) or drops (lax) an
// aspect_ratio sent with a frame image: "aspect_ratio applies only without a
// first_frame, last_frame or init image unless a reference image goes with it".
// The page names the row's own frame roles, says "unless" only where the row
// takes a reference image, and names the role a role-less image takes.
test('a ratio a frame image overrides reads as the validator applies it', () => {
  const en = translator('en');
  // vidu/viduq1, in es
  assert.equal(
    fmt.aspectRatioFramesText({ roles: ['first_frame', 'last_frame'], unlessReference: true, defaultRole: 'first_frame' }, translator('es'), 'es'),
    'una imagen de fotograma (first_frame o last_frame) fija la forma del vídeo, así que la relación de aspecto solo se aplica sin ella, salvo que la acompañe una imagen con rol reference · una imagen enviada sin rol cuenta como first_frame',
  );
  // kling/video-kling-v3
  assert.equal(
    fmt.aspectRatioFramesText({ roles: ['first_frame', 'last_frame'], unlessReference: false, defaultRole: 'first_frame' }, en, 'en'),
    'a frame image (first_frame or last_frame) sets the shape of the video, so a ratio applies only without one · an image sent without a role counts as first_frame',
  );
  // vidu/viduq1
  assert.equal(
    fmt.aspectRatioFramesText({ roles: ['first_frame', 'last_frame'], unlessReference: true, defaultRole: 'first_frame' }, en, 'en'),
    'a frame image (first_frame or last_frame) sets the shape of the video, so a ratio applies only without one, unless a reference image goes with it · an image sent without a role counts as first_frame',
  );
  // One frame role, and a role-less image that lands elsewhere
  assert.equal(
    fmt.aspectRatioFramesText({ roles: ['first_frame'], unlessReference: false }, en, 'en'),
    'a frame image (first_frame) sets the shape of the video, so a ratio applies only without one',
  );
});

test('a clip preset reads as its id, its name and the canonical preset it plays', () => {
  assert.equal(fmt.presetText({ id: '0', label: 'Idle', canonical: 'idle' }), '0 Idle (idle)');
  assert.equal(fmt.presetText({ id: '1', label: 'Walking Woman' }), '1 Walking Woman');
  assert.equal(fmt.presetText({ id: 'preset:idle', canonical: 'idle' }), 'preset:idle (idle)');
});

test('every shipped model words its size, lifecycle and limits cleanly in both locales', () => {
  for (const locale of ['en', 'es']) {
    const errors = [];
    const t = translator(locale, errors);
    for (const row of readCatalog()) {
      const m = deriveModel(row);
      const texts = [
        m.sizeRange ? fmt.sizeRangeText(m.sizeRange, t, locale) : '',
        m.lifecycle ? fmt.lifecycleText(m.lifecycle, t) : '',
        m.aspectRatioFrames ? fmt.aspectRatioFramesText(m.aspectRatioFrames, t, locale) : '',
        ...m.params.flatMap((p) => fmt.paramLimitsText(p, t, locale)),
        ...m.params.flatMap((p) => (p.presets ?? []).map((preset) => fmt.presetText(preset))),
      ];
      for (const text of texts) assert.doesNotMatch(text, /reference\.|undefined|NaN/, `${row.id}: ${text}`);
    }
    assert.deepEqual(errors, [], `${locale}: next-intl reported ${errors.length} errors`);
  }
});
