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
  // stability/outpaint: it grows a canvas and nothing else (validator.rs check_expand: 400 param_required).
  assert.deepEqual(fmt.paramLimitsText({ name: 'expand', kind: 'expand', maxPx: 2000, required: true }, en, 'en'), [
    'up to 2,000 px per side',
    'required: a request without it is refused before it is quoted',
  ]);
  assert.deepEqual(fmt.paramLimitsText({ name: 'expand', kind: 'expand', maxPx: 2000, required: true }, translator('es'), 'es'), [
    'hasta 2000 px por lado',
    'obligatorio: una petición sin él se rechaza antes de cotizarla',
  ]);
  assert.deepEqual(fmt.paramLimitsText({ name: 'layout', kind: 'layout', maxItems: 32, singleWordText: true }, en, 'en'), ['up to 32', 'one word per text box']);
  assert.deepEqual(fmt.paramLimitsText({ name: 'seed', kind: 'seed' }, en, 'en'), []);
});

// validator.rs `check_frame_shape` refuses (strict) or drops (lax) an
// aspect_ratio sent with a frame image: "aspect_ratio applies only without a
// first_frame, last_frame or init image unless a reference image goes with it".
// The page names the row's own frame roles, says "unless" only where the row
// takes a reference image beside a frame, and names the role a role-less image takes.
test('a ratio a frame image overrides reads as the validator applies it', () => {
  const en = translator('en');
  // A row whose reference image may go with a frame (none ships today: frames-refs gave every such reference a
  // not_with), in es
  assert.equal(
    fmt.aspectRatioFramesText({ roles: ['first_frame', 'last_frame'], unlessReference: true, defaultRole: 'first_frame' }, translator('es'), 'es'),
    'una imagen de fotograma (first_frame o last_frame) fija la forma del vídeo, así que la relación de aspecto solo se aplica sin ella, salvo que la acompañe una imagen con rol reference · una imagen enviada sin rol cuenta como first_frame',
  );
  // kling/video-kling-v3
  assert.equal(
    fmt.aspectRatioFramesText({ roles: ['first_frame', 'last_frame'], unlessReference: false, defaultRole: 'first_frame' }, en, 'en'),
    'a frame image (first_frame or last_frame) sets the shape of the video, so a ratio applies only without one · an image sent without a role counts as first_frame',
  );
  // The same row, in en
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

// vidu-mix: Vidu takes first/last frames or reference images, never both, so
// its reference role names the frame roles in `not_with` and the mix is
// refused before the ratio is read. The page says no "unless" for it.
test('a ratio on a row whose reference image is never sent with a frame reads without the unless', () => {
  const [row] = readCatalog().filter((r) => r.id === 'vidu/viduq1');
  assert.ok(row, 'vidu/viduq1 is in the catalog');
  assert.equal(
    fmt.aspectRatioFramesText(deriveModel(row).aspectRatioFrames, translator('en'), 'en'),
    'a frame image (first_frame or last_frame) sets the shape of the video, so a ratio applies only without one · an image sent without a role counts as first_frame',
  );
});

// validator.rs `check_refs` refuses an image of a role sent with one of a role
// it names in `not_with` (400 ref_role_conflict, in either mode): "'vidu/viduq1'
// takes reference or first_frame images, not both in one request".
test('roles a model never takes together read as one line per role that names them', () => {
  const en = translator('en');
  const frames = [
    { name: 'first_frame', required: false, min: 0, max: 1 },
    { name: 'last_frame', required: false, min: 0, max: 1 },
  ];
  // vidu/viduq1
  const viduq1 = [...frames, { name: 'reference', required: false, min: 0, max: 7, notWith: ['first_frame', 'last_frame'] }];
  assert.deepEqual(fmt.notWithText(viduq1, en, 'en'), [
    "reference images can't be sent with first_frame or last_frame images: the vendor takes one or the other",
  ]);
  assert.deepEqual(fmt.notWithText(viduq1, translator('es'), 'es'), [
    'las imágenes con rol reference no pueden ir en la misma petición que las de rol first_frame o last_frame: el proveedor admite unas u otras',
  ]);
  // vidu/viduq4-preview
  assert.deepEqual(fmt.notWithText([frames[0], { name: 'reference', required: false, min: 0, max: 15, notWith: ['first_frame'] }], en, 'en'), [
    "reference images can't be sent with first_frame images: the vendor takes one or the other",
  ]);
  // No role names another: nothing to say.
  assert.deepEqual(fmt.notWithText([...frames, { name: 'reference', required: false, min: 0, max: 3 }], en, 'en'), []);
});

// validator.rs `check_video_request` refuses a last_frame image sent without a
// first_frame (or init) image where the last_frame role sets `needs_first_frame`
// (400 ref_role_required, in either mode): "'ideogram/kling-3-standard' ends a
// clip on a last_frame only after a first_frame; send a first_frame image too".
test('a last frame that needs a first frame reads as one line that names the roles it needs', () => {
  const en = translator('en');
  const first = { name: 'first_frame', required: false, min: 0, max: 1 };
  // ideogram/kling-3-standard
  const kling = [first, { name: 'last_frame', required: false, min: 0, max: 1, needsFirstFrame: true }];
  assert.deepEqual(fmt.needsFirstFrameText(kling, en, 'en'), [
    'last_frame images need a first_frame image in the same request: the vendor ends a clip on a last frame only after a first frame',
  ]);
  assert.deepEqual(fmt.needsFirstFrameText(kling, translator('es'), 'es'), [
    'las imágenes con rol last_frame necesitan una imagen con rol first_frame en la misma petición: el proveedor solo cierra un clip con un último fotograma si lo abre con un primero',
  ]);
  // A row that starts a clip from an init image too: either one goes with the last frame.
  assert.deepEqual(fmt.needsFirstFrameText([{ name: 'init', required: false, min: 0, max: 1 }, ...kling], en, 'en'), [
    'last_frame images need a first_frame or init image in the same request: the vendor ends a clip on a last frame only after a first frame',
  ]);
  // luma/ray-2: a last frame alone goes, so nothing to say.
  assert.deepEqual(fmt.needsFirstFrameText([first, { name: 'last_frame', required: false, min: 0, max: 1 }], en, 'en'), []);
});

// validator.rs `check_video_request` refuses a reference image sent without a
// first_frame (or init) image where the reference role sets `needs_first_frame`
// (400 ref_role_required, in either mode): "'runway/klingO3_pro' takes reference
// images only on image-to-video; send a first_frame image too".
test('a reference that needs a first frame reads as a line of its own, with its own reason', () => {
  const en = translator('en');
  const first = { name: 'first_frame', required: false, min: 0, max: 1 };
  const last = { name: 'last_frame', required: false, min: 0, max: 1, needsFirstFrame: true };
  const lastLine = 'last_frame images need a first_frame image in the same request: the vendor ends a clip on a last frame only after a first frame';
  // runway/klingO3_pro
  const o3 = [first, last, { name: 'reference', required: false, min: 0, max: 7, needsFirstFrame: true }];
  assert.deepEqual(fmt.needsFirstFrameText(o3, en, 'en'), [
    lastLine,
    'reference images need a first_frame image in the same request: the vendor takes reference images only in a clip that starts from a first frame',
  ]);
  assert.deepEqual(fmt.needsFirstFrameText(o3, translator('es'), 'es'), [
    'las imágenes con rol last_frame necesitan una imagen con rol first_frame en la misma petición: el proveedor solo cierra un clip con un último fotograma si lo abre con un primero',
    'las imágenes con rol reference necesitan una imagen con rol first_frame en la misma petición: el proveedor solo admite imágenes de referencia en un clip que parte de un primer fotograma',
  ]);
  // runway/kling_4: its references go without a frame too, so only the last frame's line.
  assert.deepEqual(fmt.needsFirstFrameText([first, last, { name: 'reference', required: false, min: 0, max: 10 }], en, 'en'), [lastLine]);
});

// `returned_together` (schema.rs): the containers one vendor job returns
// together. A generation that keeps nothing (keep_outputs: false) converts
// nothing, so it is promised only what its job returns; anything more is
// refused, 400 format_needs_keeping.
test('a format set reads as the containers one vendor job returns together', () => {
  // meshy/rig's output_formats
  const rig = { name: 'output_formats', kind: 'string_array', enum: ['glb', 'fbx'], default: ['glb'], returnedTogether: ['glb', 'fbx'] };
  assert.deepEqual(fmt.paramLimitsText(rig, translator('en'), 'en'), [
    'one vendor job returns glb and fbx together; a generation that keeps nothing (keep_outputs: false) gets only what its job returns',
  ]);
  assert.deepEqual(fmt.paramLimitsText(rig, translator('es'), 'es'), [
    'un trabajo del proveedor devuelve glb y fbx a la vez; una generación que no guarda nada (keep_outputs: false) recibe solo lo que devuelve su trabajo',
  ]);
  // tripo/rig: a job returns the one container it is asked for
  assert.deepEqual(fmt.paramLimitsText({ name: 'output_formats', kind: 'string_array', enum: ['glb', 'fbx'], default: ['glb'] }, translator('en'), 'en'), []);
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
        ...fmt.notWithText(m.refRoles, t, locale),
        ...fmt.needsFirstFrameText(m.refRoles, t, locale),
        ...m.params.flatMap((p) => fmt.paramLimitsText(p, t, locale)),
        ...m.params.flatMap((p) => (p.presets ?? []).map((preset) => fmt.presetText(preset))),
      ];
      for (const text of texts) assert.doesNotMatch(text, /reference\.|undefined|NaN/, `${row.id}: ${text}`);
    }
    assert.deepEqual(errors, [], `${locale}: next-intl reported ${errors.length} errors`);
  }
});
