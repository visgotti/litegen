import { describe, expect, it, vi } from 'vitest';

// SingleMode pulls in the shared SDK client (whose module body reads
// localStorage) through the component tree; these tests cover the pure
// request-body builder, so the real client is never needed.
vi.mock('../sdk-client', () => ({ client: {} }));

const { buildGenerationBody, effectiveSize, omitImageOnlyParams } = await import('./SingleMode');

describe('effectiveSize', () => {
  const options = ['512x512', '1024x1024'];

  it("keeps the user's pick while the model offers it", () => {
    expect(effectiveSize('1024x1024', options)).toBe('1024x1024');
  });

  it("falls back to the model's first size when the pick is unset or not offered", () => {
    // A switch between models must never carry a size the new one lacks.
    expect(effectiveSize('', options)).toBe('512x512');
    expect(effectiveSize('2048x2048', options)).toBe('512x512');
  });

  it('passes a typed size through for a model with free-form sizes', () => {
    expect(effectiveSize('640x480', [])).toBe('640x480');
    expect(effectiveSize('', [])).toBe('');
  });
});

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

  it('keeps negative_prompt and seed for a model that declares them', () => {
    const body = buildGenerationBody(
      { ...form, model: 'mock/all-params-image', negativePrompt: 'blurry', seed: '7' },
      'image',
      'required',
      { negative_prompt: {}, seed: {} },
    );
    expect(body).toMatchObject({
      model: 'mock/all-params-image',
      prompt: 'a stone gargoyle',
      negative_prompt: 'blurry',
      seed: 7,
      strict: true,
    });
  });

  // mock/mesh-3d declares neither, and no 3D model takes a negative prompt: in
  // strict mode a value left over from an image model would be a 400.
  it('drops negative_prompt and seed for a model that does not declare them', () => {
    const body = buildGenerationBody(
      { ...form, negativePrompt: 'blurry', seed: '7' },
      'model3d',
      'required',
      { target_polycount: {} },
    );
    expect(body).not.toHaveProperty('negative_prompt');
    expect(body).not.toHaveProperty('seed');
    expect(body).toMatchObject({ model: 'mock/mesh-3d', prompt: 'a stone gargoyle', strict: true });
  });

  it('sends them as entered while the schema is still loading', () => {
    const body = buildGenerationBody({ ...form, negativePrompt: 'blurry', seed: '7' }, 'model3d');
    expect(body).toMatchObject({ negative_prompt: 'blurry', seed: 7 });
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

  // `prompt.max_length == 0` (fal/trellis, fal/hunyuan3d-v2): a prompt left
  // over from the previously selected model would be a 400 in strict mode.
  it('sends no prompt to a model that takes none', () => {
    expect(buildGenerationBody(form, 'model3d', 'unused')).not.toHaveProperty('prompt');
  });

  it('leaves a blank prompt off the wire rather than sending ""', () => {
    expect(buildGenerationBody({ ...form, prompt: '  ' }, 'model3d', 'optional')).not.toHaveProperty('prompt');
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
