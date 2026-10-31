import { describe, expect, it, vi } from 'vitest';
import type { ModelSchema } from '@litegen/sdk';
import { derivedParams, fanOutJobs, mergeParams, refsForModel, schemaInputs, wireValue } from './params';
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

  it('sends a time range only when it ends after it starts', () => {
    const spec = { kind: 'time_range' };
    expect(wireValue(spec, { start_s: '0.5', end_s: '1.5' })).toEqual({ start_s: 0.5, end_s: 1.5 });
    expect(wireValue(spec, { start_s: '2', end_s: '1' })).toBeUndefined();
    expect(wireValue(spec, { start_s: '1' })).toBeUndefined();
  });

  it('sends a palette as distinct #rrggbb colours, at most max_colors', () => {
    const spec = { kind: 'palette', max_colors: 2 };
    expect(wireValue(spec, ['#FF0000', '#ff0000', 'red', '#00ff00', '#0000ff'])).toEqual(['#ff0000', '#00ff00']);
    expect(wireValue(spec, ['nope'])).toBeUndefined();
  });

  it('sends layout boxes as fractions, words with their text', () => {
    const spec = { kind: 'layout', max_items: 8 };
    expect(wireValue(spec, [
      { kind: 'text', text: 'SALE', x: '0.1', y: '0.1', w: '0.5', h: '0.2' },
      { kind: 'object', prompt: 'a red bicycle', x: '0.2', y: '0.4', w: '0.6', h: '0.5' },
      { kind: 'text', text: '', x: '0', y: '0', w: '0.5', h: '0.5' },
      { kind: 'object', x: '0.5', y: '0.5', w: '0.7', h: '0.2' },
    ])).toEqual([
      { kind: 'text', text: 'SALE', box: { x: 0.1, y: 0.1, w: 0.5, h: 0.2 } },
      { kind: 'object', prompt: 'a red bicycle', box: { x: 0.2, y: 0.4, w: 0.6, h: 0.5 } },
    ]);
  });

  it('sends each animation by a name the server takes, with a preset or a prompt', () => {
    const spec = { kind: 'animation_list', max_items: 5, presets: [{ id: 'mock:walk', canonical: 'walk' }, { id: 'mock:wave' }] };
    expect(wireValue(spec, [
      { name: '', preset: 'mock:walk', loop: true },
      { name: 'jump', prompt: 'a big jump', duration_s: '1.5' },
      { name: '', preset: 'mock:wave' },
      { name: '', preset: 'mock:walk' },
      { name: 'bad name!', preset: 'mock:walk' },
      { name: 'nothing' },
    ])).toEqual([
      // Named for the canonical preset, else the preset id made legal; a
      // repeat gets a suffix (names must be 1–64 of A–Z a–z 0–9 _ . -, unique).
      { name: 'walk', preset: 'mock:walk', loop: true },
      { name: 'jump', prompt: 'a big jump', duration_s: 1.5 },
      { name: 'mock_wave', preset: 'mock:wave' },
      { name: 'walk-2', preset: 'mock:walk' },
    ]);
  });

  it('sends a source by generation id or URL, with its format', () => {
    expect(wireValue({ kind: 'source_model' }, { type: 'generation', value: ' gen_123 ' })).toEqual({ type: 'generation', value: 'gen_123' });
    expect(wireValue({ kind: 'source_video' }, { type: 'url', value: 'https://x/clip.gif', format: 'gif' }))
      .toEqual({ type: 'url', value: 'https://x/clip.gif', format: 'gif' });
    expect(wireValue({ kind: 'source_model' }, { type: 'generation', value: '' })).toBeUndefined();
  });

  it('sends segmentation prompts of the kinds the model takes', () => {
    const spec = { kind: 'segmentation_prompts', types: ['point', 'text'], max_items: 16 };
    expect(wireValue(spec, [
      { type: 'point', x: '12', y: '30', label: '1' },
      { type: 'box', x: '0', y: '0', w: '10', h: '10' },
      { type: 'text', text: 'the cat' },
      { type: 'point', x: '', y: '3' },
    ])).toEqual([
      { type: 'point', x: 12, y: 30, label: 1 },
      { type: 'text', text: 'the cat' },
    ]);
  });
});

describe('fields a model takes without declaring them', () => {
  const masked = schema({ params: {}, ref_inputs: { roles: { init: {}, mask: {} } } });
  const regioned = schema({ params: { region: { kind: 'region' } } });
  const plain = schema({ params: { steps: { kind: 'int' } } });

  it('offers mask growth and keeping unmasked pixels wherever a mask or a region goes', () => {
    expect(Object.keys(derivedParams(masked)).sort()).toEqual(['feather_px', 'mask_grow_px', 'preserve_unmasked']);
    expect(Object.keys(derivedParams(regioned)).sort()).toEqual(['feather_px', 'mask_grow_px', 'preserve_unmasked']);
    expect(derivedParams(plain)).toEqual({});
    expect(mergeParams({ m: masked }).map(p => p.name)).toContain('mask_grow_px');
  });

  it('sends them to the models that take them only', () => {
    const shared = { params: { mask_grow_px: '8', preserve_unmasked: true } };
    expect(schemaInputs(masked, shared)).toEqual({ mask_grow_px: 8, preserve_unmasked: true });
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
    const refs: RefEntry[] = [{ id: '1', role: 'image', type: 'url', value: 'https://x/a.png' }];
    expect(buildSegmentationBody('mock/segment-image', spec, { params: { prompts: [{ type: 'text', text: 'the cat' }] }, refs })).toEqual({
      body: { model: 'mock/segment-image', image: 'https://x/a.png', prompts: [{ type: 'text', text: 'the cat' }] },
      problems: [],
    });
    expect(buildSegmentationBody('mock/segment-image', spec, { params: {}, refs: [] }).problems)
      .toEqual(['needs an image to segment', 'needs a point, box or text prompt']);
    // "a text prompt selects every instance of what it names and cannot be
    // mixed with point or box prompts".
    const mixed = { params: { prompts: [{ type: 'point', x: '1', y: '2' }, { type: 'text', text: 'cat' }] }, refs };
    expect(buildSegmentationBody('mock/segment-image', spec, mixed).problems)
      .toEqual(['takes a text prompt alone, not with point or box prompts or a second text prompt']);
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
});
