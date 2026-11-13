import { describe, expect, it, vi } from 'vitest';
import type { ModelSchema } from '@litegen/sdk';
import { fanOutJobs, mergeParams, paramsOf, refsForModel, schemaInputs, wireValue } from './params';
import type { RefEntry } from './types';

// SingleMode's module tree reads localStorage through the shared SDK client;
// only its pure body builder is under test here.
vi.mock('../sdk-client', () => ({ client: {} }));
const { buildGenerationBody, buildSegmentationBody, runKindOf } = await import('./SingleMode');

const schema = (s: Record<string, unknown>) => s as unknown as ModelSchema;

describe('wireValue: what a structured control sends', () => {
  it('sends a region only once all four sides are whole pixels', () => {
    const spec = { kind: 'region' };
    expect(wireValue(spec, { x: '10', y: '20', w: '300', h: '256' })).toEqual({ x: 10, y: 20, w: 300, h: 256 });
    expect(wireValue(spec, { x: '10', y: '20', w: '300' })).toBeUndefined();
    expect(wireValue(spec, { x: '10', y: '20', w: '0', h: '256' })).toBeUndefined();
    expect(wireValue(spec, { x: '1.5', y: '20', w: '300', h: '256' })).toBeUndefined();
    expect(wireValue(spec, '')).toBeUndefined();
  });

  it('sends the sides of an expand that grow, and nothing when none does', () => {
    const spec = { kind: 'expand' };
    expect(wireValue(spec, { left: '64', right: '64' })).toEqual({ left: 64, top: 0, right: 64, bottom: 0 });
    expect(wireValue(spec, { left: '', top: '0' })).toBeUndefined();
    expect(wireValue(spec, { left: '-4' })).toBeUndefined();
  });

  // N4: a value is sent as typed once it is complete; whether it is a window,
  // a colour or inside the canvas is the server's to say, in its words.
  it('sends a time range as typed once both ends are numbers: the server judges the window', () => {
    const spec = { kind: 'time_range' };
    expect(wireValue(spec, { start_s: '0.5', end_s: '1.5' })).toEqual({ start_s: 0.5, end_s: 1.5 });
    expect(wireValue(spec, { start_s: '2', end_s: '1' })).toEqual({ start_s: 2, end_s: 1 });
    expect(wireValue(spec, { start_s: '1' })).toBeUndefined();
  });

  it('sends a palette as typed, every colour filled in: the server judges each and the count', () => {
    const spec = { kind: 'palette', max_colors: 2 };
    expect(wireValue(spec, ['#FF0000', '#ff0000', 'red', ' ', '#00ff00'])).toEqual(['#FF0000', '#ff0000', 'red', '#00ff00']);
    expect(wireValue(spec, [''])).toBeUndefined();
  });

  it('sends layout boxes as typed fractions once all four are numbers, words with their text', () => {
    const spec = { kind: 'layout', max_items: 8 };
    expect(wireValue(spec, [
      { kind: 'text', text: 'SALE', x: '0.1', y: '0.1', w: '0.5', h: '0.2' },
      { kind: 'object', prompt: 'a red bicycle', x: '0.2', y: '0.4', w: '0.6', h: '0.5' },
      { kind: 'text', text: '', x: '0', y: '0', w: '0.5', h: '0.5' },
      { kind: 'object', x: '0.5', y: '0.5', w: '0.7', h: '0.2' },
      { kind: 'object', x: '0.5', y: '0.5', w: '', h: '0.2' },
    ])).toEqual([
      { kind: 'text', text: 'SALE', box: { x: 0.1, y: 0.1, w: 0.5, h: 0.2 } },
      { kind: 'object', prompt: 'a red bicycle', box: { x: 0.2, y: 0.4, w: 0.6, h: 0.5 } },
      { kind: 'object', box: { x: 0.5, y: 0.5, w: 0.7, h: 0.2 } },
    ]);
  });

  it('sends a clip name as typed, and none for a blank one: the server names and judges it', () => {
    // E11c: litegen names a clip sent without one (its canonical preset,
    // `clip-<n>`, `-2` on a repeat) and refuses an illegal or repeated name
    // in its own words; the page invents and drops nothing.
    const spec = { kind: 'animation_list', max_items: 5, presets: [{ id: 'mock:walk', canonical: 'walk' }, { id: 'mock:wave' }] };
    expect(wireValue(spec, [
      { name: '', preset: 'mock:walk', loop: true },
      { name: 'bad name!', preset: 'mock:wave' },
      { name: 'walk', preset: 'mock:walk' },
      { name: 'jump', prompt: 'a big jump', duration_s: '1.5' },
      { name: 'nothing' },
    ])).toEqual([
      { preset: 'mock:walk', loop: true },
      { name: 'bad name!', preset: 'mock:wave' },
      { name: 'walk', preset: 'mock:walk' },
      { name: 'jump', prompt: 'a big jump', duration_s: 1.5 },
    ]);
  });

  it('sends a source by generation id or URL, with its format', () => {
    expect(wireValue({ kind: 'source_model' }, { type: 'generation', value: ' gen_123 ' })).toEqual({ type: 'generation', value: 'gen_123' });
    expect(wireValue({ kind: 'source_video' }, { type: 'url', value: 'https://x/clip.gif', format: 'gif' }))
      .toEqual({ type: 'url', value: 'https://x/clip.gif', format: 'gif' });
    expect(wireValue({ kind: 'source_model' }, { type: 'generation', value: '' })).toBeUndefined();
  });

  it('sends a URL source with the format its select shows, even one never changed', () => {
    // A one-format select never fires onChange: what it shows is still sent,
    // or the server refuses the source (source_format_required).
    expect(wireValue({ kind: 'source_model', formats: ['glb'] }, { type: 'url', value: 'https://x/mesh.glb' }))
      .toEqual({ type: 'url', value: 'https://x/mesh.glb', format: 'glb' });
    expect(wireValue({ kind: 'source_video', formats: ['gif', 'mp4'] }, { type: 'url', value: 'https://x/clip.gif' }))
      .toEqual({ type: 'url', value: 'https://x/clip.gif', format: 'gif' });
  });

  it("sends segmentation prompts of the kinds the model takes, a point's label as typed", () => {
    const spec = { kind: 'segmentation_prompts', types: ['point', 'text'], max_items: 16 };
    expect(wireValue(spec, [
      { type: 'point', x: '12', y: '30', label: '1' },
      { type: 'box', x: '0', y: '0', w: '10', h: '10' },
      { type: 'text', text: 'the cat' },
      { type: 'point', x: '', y: '3' },
      { type: 'point', x: '5', y: '6', label: '2' },
      { type: 'point', x: '7', y: '8', label: '' },
    ])).toEqual([
      { type: 'point', x: 12, y: 30, label: 1 },
      { type: 'text', text: 'the cat' },
      { type: 'point', x: 5, y: 6, label: 2 },
      { type: 'point', x: 7, y: 8 },
    ]);
  });
});

describe('fields litegen applies, as the server serves them', () => {
  // As GET /v1/models/{id} serves them (E11a): the fields and their bounds are
  // the server's, in `litegen_params`; the page derives none from a shape.
  const maskFields = {
    mask_grow_px: { kind: 'int', min: 0, max: 100, label: 'Mask grow (px)' },
    preserve_unmasked: { kind: 'bool', default: false, label: 'Keep unmasked pixels' },
    feather_px: { kind: 'int', min: 0, max: 256, label: 'Feather (px)' },
  };
  const masked = schema({ media_type: 'image', params: {}, ref_inputs: { roles: { init: {}, mask: {} } }, litegen_params: maskFields });
  const regioned = schema({ media_type: 'image', params: { region: { kind: 'region' } }, litegen_params: maskFields });
  const plain = schema({ media_type: 'image', params: { steps: { kind: 'int' } } });
  const mesh = schema({
    media_type: 'model3d',
    params: {},
    litegen_params: { ground: { kind: 'bool', default: false }, center: { kind: 'bool', default: false }, height_m: { kind: 'float', max: 10000 } },
  });

  it('takes each field and its bounds from litegen_params', () => {
    const served = { ground: { kind: 'bool', default: false }, height_m: { kind: 'float', max: 12000 } };
    expect(paramsOf(schema({ media_type: 'model3d', params: {}, litegen_params: served }))).toEqual(served);
    expect(mergeParams({ m: masked }).map(p => p.name)).toContain('mask_grow_px');
  });

  it("derives nothing from the model's shape", () => {
    expect(paramsOf(schema({ media_type: 'image', params: {}, ref_inputs: { roles: { init: {}, mask: {} } } }))).toEqual({});
    expect(paramsOf(schema({ media_type: 'model3d', params: {} }))).toEqual({});
  });

  it('sends a typed height as typed; the server says what it takes', () => {
    expect(schemaInputs(mesh, { params: { height_m: '0.0005' } })).toEqual({ height_m: 0.0005 });
    expect(schemaInputs(mesh, { params: { height_m: '0' } })).toEqual({ height_m: 0 });
    expect(schemaInputs(mesh, { params: { ground: true, center: true, height_m: '1.8' } })).toEqual({ ground: true, center: true, height_m: 1.8 });
  });

  it('lets a declared param keep its own spec', () => {
    const rig = schema({
      media_type: 'model3d',
      params: { height_m: { kind: 'float', min: 0.5, max: 3, default: 1.7 } },
      litegen_params: { ground: { kind: 'bool', default: false }, height_m: { kind: 'float', max: 10000 } },
    });
    expect(paramsOf(rig).height_m).toEqual({ kind: 'float', min: 0.5, max: 3, default: 1.7 });
  });

  it('sends them to the models that take them only', () => {
    const shared = { params: { mask_grow_px: '8', preserve_unmasked: true } };
    expect(schemaInputs(masked, shared)).toEqual({ mask_grow_px: 8, preserve_unmasked: true });
    expect(schemaInputs(regioned, shared)).toEqual({ mask_grow_px: 8, preserve_unmasked: true });
    expect(schemaInputs(plain, shared)).toEqual({});
  });

  it('sends a structured param in its wire form, and an incomplete one not at all', () => {
    expect(schemaInputs(regioned, { params: { region: { x: '1', y: '2', w: '300', h: '300' } } }))
      .toEqual({ region: { x: 1, y: 2, w: 300, h: 300 } });
    expect(schemaInputs(regioned, { params: { region: { x: '1' } } })).toEqual({});
  });
});

describe('keyframe times', () => {
  it('sends each keyframe its time, and no other role one', () => {
    const s = schema({ ref_inputs: { roles: { keyframe: { max_count: 8 }, first_frame: { max_count: 1 } } } });
    const refs: RefEntry[] = [
      { id: '1', role: 'keyframe', type: 'url', value: 'https://x/a.png', time_s: '1.25' },
      { id: '2', role: 'keyframe', type: 'url', value: 'https://x/b.png', time_s: '' },
      { id: '3', role: 'first_frame', type: 'url', value: 'https://x/c.png', time_s: '2' },
    ];
    expect(refsForModel(s, refs)).toEqual([
      { type: 'url', value: 'https://x/a.png', role: 'keyframe', time_s: 1.25 },
      { type: 'url', value: 'https://x/b.png', role: 'keyframe' },
      { type: 'url', value: 'https://x/c.png', role: 'first_frame' },
    ]);
  });
});

describe('a video request', () => {
  it('carries neither n nor size, which only images take', () => {
    const body = buildGenerationBody(
      { model: 'mock/sprite-video', prompt: 'a square', negativePrompt: '', seed: '', size: '1024x1024', strict: true },
      'video',
    );
    expect(body).toEqual({ model: 'mock/sprite-video', prompt: 'a square', strict: true });
  });
});

describe('what a run is', () => {
  const info = (m: Record<string, unknown>) => m as unknown as Parameters<typeof runKindOf>[0];

  it('runs a segmentation model at its own endpoint, and video and 3D models as jobs', () => {
    expect(runKindOf(info({ media_type: 'image', capabilities: { task: 'segment' } }))).toBe('segment');
    expect(runKindOf(info({ media_type: 'image', capabilities: { task: 'generate' } }))).toBe('image');
    expect(runKindOf(info({ media_type: 'video', capabilities: { task: 'extend' } }))).toBe('video');
    expect(runKindOf(info({ media_type: 'model3d', capabilities: { task: 'rig' } }))).toBe('model3d');
  });

  it('builds a segmentation request from the image and its prompts, or says what is missing', () => {
    const spec = { kind: 'segmentation_prompts', types: ['point', 'text'], max_items: 4 } as unknown as Parameters<typeof buildSegmentationBody>[1];
    const refs: RefEntry[] = [{ id: '1', role: 'image', type: 'base64', value: 'data:image/png;base64,iVBORw0KGgo=', name: 'a.png' }];
    expect(buildSegmentationBody('mock/segment-image', spec, { params: { prompts: [{ type: 'text', text: 'the cat' }] }, refs })).toEqual({
      body: { model: 'mock/segment-image', image: 'data:image/png;base64,iVBORw0KGgo=', prompts: [{ type: 'text', text: 'the cat' }] },
      problems: [],
    });
    // The endpoint takes the image itself ("base64, or a data: URI"), never a URL.
    const byUrl: RefEntry[] = [{ id: '2', role: 'image', type: 'url', value: 'https://x/a.png' }];
    expect(buildSegmentationBody('mock/segment-image', spec, { params: { prompts: [{ type: 'text', text: 'the cat' }] }, refs: byUrl }).problems)
      .toEqual(['segments an uploaded image, not a URL']);
    expect(buildSegmentationBody('mock/segment-image', spec, { params: {}, refs: [] }).problems)
      .toEqual(['needs an image to segment', 'needs a point, box or text prompt']);
    // Which prompts go together is the server's rule ("a text prompt ...
    // cannot be mixed with point or box prompts"): the submit says so, in its
    // words; the page holds back only what is missing (E11b).
    const mixed = { params: { prompts: [{ type: 'point', x: '1', y: '2' }, { type: 'text', text: 'cat' }] }, refs };
    expect(buildSegmentationBody('mock/segment-image', spec, mixed).problems).toEqual([]);
  });
});

describe('N images in Compare mode', () => {
  it('are N requests of one image each, one tile apiece; a 3D job stays one', () => {
    const jobs = fanOutJobs([
      { modelId: 'mock/visual-image-gen', mediaType: 'image' as const, request: { model: 'mock/visual-image-gen', n: 3 } },
      { modelId: 'mock/mesh-3d', mediaType: 'model3d' as const, request: { model: 'mock/mesh-3d' } },
    ], 2);
    expect(jobs.map(j => [j.key, j.request])).toEqual([
      ['mock/visual-image-gen#0', { model: 'mock/visual-image-gen' }],
      ['mock/visual-image-gen#1', { model: 'mock/visual-image-gen' }],
      ['mock/mesh-3d#0', { model: 'mock/mesh-3d' }],
    ]);
  });

  it('give a fixed seed one step per copy, within the model\'s range: N copies of one seed are one image billed N times', () => {
    const image = (seed: number | undefined, seedMax?: number) => ({
      modelId: 'mock/visual-image-gen', mediaType: 'image' as const, seedMax,
      request: seed === undefined ? { model: 'mock/visual-image-gen' } : { model: 'mock/visual-image-gen', seed },
    });
    const seeds = (jobs: Array<{ request: object }>) => jobs.map(j => (j.request as { seed?: number }).seed);
    expect(seeds(fanOutJobs([image(7)], 3))).toEqual([7, 8, 9]);
    expect(seeds(fanOutJobs([image(7, 8)], 3))).toEqual([7, 8, 0]);
    expect(seeds(fanOutJobs([image(undefined)], 2))).toEqual([undefined, undefined]);
  });
});
