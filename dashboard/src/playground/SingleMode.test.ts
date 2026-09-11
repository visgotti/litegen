import { describe, expect, it, vi } from 'vitest';

// SingleMode pulls in the shared SDK client (whose module body reads
// localStorage) through the component tree; these tests cover the pure
// request-body builder, so the real client is never needed.
vi.mock('../sdk-client', () => ({ client: {} }));

const { buildGenerationBody, omitImageOnlyParams } = await import('./SingleMode');

const form = {
  model: 'mock/mesh-3d',
  prompt: 'a stone gargoyle',
  negativePrompt: '',
  seed: '',
  size: '1024x1024',
  n: 4,
  strict: true,
};

describe('buildGenerationBody', () => {
  it('sends size and n for an image model', () => {
    const body = buildGenerationBody({ ...form, model: 'mock/visual-image-gen' }, 'image');
    expect(body.size).toBe('1024x1024');
    expect(body.n).toBe(4);
  });

  // Model3dGenerationRequest has no `size` at all and ignores `n`: serde drops
  // `size` silently even with strict:true, so the checkbox's promise of a
  // rejection is never kept and the user's "4 meshes" quietly becomes one.
  it('omits size and n for a 3D model', () => {
    const body = buildGenerationBody(form, 'model3d');
    expect(body).not.toHaveProperty('size');
    expect(body).not.toHaveProperty('n');
  });

  it('keeps the params 3D does accept', () => {
    const body = buildGenerationBody(
      { ...form, negativePrompt: 'blurry', seed: '7' },
      'model3d',
    );
    expect(body).toMatchObject({
      model: 'mock/mesh-3d',
      prompt: 'a stone gargoyle',
      negative_prompt: 'blurry',
      seed: 7,
      strict: true,
    });
  });

  it('leaves optional params out when they are blank', () => {
    const body = buildGenerationBody({ ...form, size: '' }, 'image');
    expect(body).not.toHaveProperty('negative_prompt');
    expect(body).not.toHaveProperty('seed');
    expect(body).not.toHaveProperty('size');
  });

  it('carries strict through unchanged', () => {
    expect(buildGenerationBody({ ...form, strict: false }, 'model3d').strict).toBe(false);
  });
});

describe('omitImageOnlyParams', () => {
  // Resubmitting a history entry recorded before this fix must not put the
  // dropped fields back on the wire.
  it('drops size and n from a stored history body', () => {
    expect(omitImageOnlyParams({ model: 'mock/mesh-3d', prompt: 'x', size: '1024x1024', n: 4 }))
      .toEqual({ model: 'mock/mesh-3d', prompt: 'x' });
  });

  it('keeps every other field, including ones 3D does accept', () => {
    expect(omitImageOnlyParams({ prompt: 'x', seed: 7, strict: true, target_polycount: 20000 }))
      .toEqual({ prompt: 'x', seed: 7, strict: true, target_polycount: 20000 });
  });

  it('does not mutate the stored entry', () => {
    const stored = { prompt: 'x', n: 4 };
    omitImageOnlyParams(stored);
    expect(stored).toEqual({ prompt: 'x', n: 4 });
  });
});
