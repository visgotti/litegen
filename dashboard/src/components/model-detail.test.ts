import { describe, expect, it } from 'vitest';
import type { ModelInfo, ModelSchema } from '@litegen/sdk';
import {
  EXAMPLE_IMAGE_URL,
  buildCurl,
  capabilityLabels,
  describePrompt,
  describeSpec,
  exampleBody,
  generationEndpoint,
  refRoleRows,
} from './model-detail';

describe('capabilityLabels', () => {
  it('labels the 3D capability flags a model3d model reports', () => {
    const labels = capabilityLabels({
      supports_text_to_3d: true,
      supports_image_to_3d: true,
      supports_multiview_to_3d: true,
      supports_texture: true,
      supports_pbr: true,
      supports_rig: true,
    });
    expect(labels).toEqual([
      'Text → 3D',
      'Image → 3D',
      'Multiview → 3D',
      'Textures',
      'PBR materials',
      'Auto-rigging',
    ]);
  });

  it('still labels the image and video flags', () => {
    expect(capabilityLabels({ supports_text_to_image: true, supports_inpainting: true }))
      .toEqual(['Text → image', 'Inpainting']);
    expect(capabilityLabels({ supports_image_to_video: true })).toEqual(['Image → video']);
  });

  it('skips flags that are false or absent', () => {
    expect(capabilityLabels({ supports_text_to_3d: false, supports_image_to_image: false })).toEqual([]);
    expect(capabilityLabels({})).toEqual([]);
  });

  it('tolerates a missing capabilities object', () => {
    expect(capabilityLabels(undefined)).toEqual([]);
    expect(capabilityLabels(null)).toEqual([]);
  });

  it('ignores keys that are not capability flags', () => {
    expect(capabilityLabels({ supported_sizes: ['1024x1024'], output_formats: ['glb'] })).toEqual([]);
  });
});

// Shaped like GET /v1/models/{id}'s fal 3D rows; the format list is a fixture
// for the rule (every format derivable), not a copy of today's catalog.
const trellis = {
  id: 'fal/trellis',
  // `max_length: 0`: the server's "this model takes no prompt".
  prompt: { required: false, max_length: 0 },
  capabilities: { image_to_3d: true },
  ref_inputs: { max_total: 1, default_role: 'init', roles: { init: { required: true, min_count: 1, max_count: 1 } } },
  params: {
    output_formats: {
      kind: 'string_array',
      enum_values: ['glb', 'obj', 'stl', 'ply'],
      derivable_values: ['glb', 'obj', 'stl', 'ply'],
      max_items: 1,
    },
  },
} as unknown as ModelSchema;

const rodin = {
  id: 'fal/hyper3d-rodin',
  prompt: { required: false, max_length: 1024 },
  capabilities: { text_to_3d: true, image_to_3d: true },
  ref_inputs: { max_total: 5, roles: { init: { required: false, min_count: 0, max_count: 5 } } },
} as unknown as ModelSchema;

const info = (id: string, media_type: string) => ({ id, media_type }) as unknown as ModelInfo;

describe('describeSpec', () => {
  it('says any combination is one job when every format shares it', () => {
    // The common case since litegen converts between all its containers.
    expect(describeSpec((trellis as unknown as { params: Record<string, Record<string, unknown>> }).params.output_formats))
      .toBe('string_array: glb | obj | stl | ply · any combination is one provider job');
  });

  it('states the provider-job budget when only some formats share the job', () => {
    expect(describeSpec({
      kind: 'string_array',
      enum_values: ['glb', 'usdz', 'fbx', 'obj'],
      derivable_values: ['glb', 'obj'],
      max_items: 1,
    })).toBe('string_array: glb | usdz | fbx | obj · 1 provider job (glb/obj share one)');
  });

  it('describes a format set the server names no shared job for', () => {
    expect(describeSpec({ kind: 'string_array', enum_values: ['glb', 'fbx'], max_items: 2 }))
      .toBe('string_array: glb | fbx · 2 provider jobs');
    expect(describeSpec({ kind: 'string_array', enum_values: ['glb'] })).toBe('string_array: glb');
  });

  it('keeps the existing summaries for other kinds', () => {
    expect(describeSpec({ kind: 'string', enum_values: ['triangle', 'quad'] })).toBe('string: triangle | quad');
    expect(describeSpec({ kind: 'int', min: 100, max: 300000 })).toBe('int 100–300000');
    expect(describeSpec({ kind: 'bool', default: true })).toBe('bool (default true)');
    expect(describeSpec({})).toBe('value');
  });
});

describe('describePrompt', () => {
  it('says an image-only model does not use the prompt', () => {
    expect(describePrompt(trellis)).toBe('Not used — this model generates from the input image');
  });

  it('says a text-or-image model takes it optionally, with its limit', () => {
    expect(describePrompt(rodin)).toBe('Optional when a reference image is attached · up to 1,024 characters');
  });

  it('says required otherwise', () => {
    expect(describePrompt({ prompt: { required: true } } as unknown as ModelSchema)).toBe('Required');
  });
});

describe('refRoleRows', () => {
  it('states each role in words', () => {
    expect(refRoleRows(trellis)).toEqual([{ role: 'init', label: 'Input image', rule: 'required · exactly 1' }]);
    expect(refRoleRows(rodin)).toEqual([{ role: 'init', label: 'Input image', rule: 'optional · up to 5' }]);
  });

  it('is empty for a model without image inputs, or before the schema loads', () => {
    expect(refRoleRows({} as ModelSchema)).toEqual([]);
    expect(refRoleRows(null)).toEqual([]);
  });
});

describe('exampleBody (Copy as curl)', () => {
  it('gives an image-only model its required image and no prompt', () => {
    // The old example — a bare prompt — is a 400 on TRELLIS.
    expect(exampleBody(info('fal/trellis', 'model3d'), trellis)).toEqual({
      model: 'fal/trellis',
      reference_images: [{ type: 'url', value: EXAMPLE_IMAGE_URL, role: 'init' }],
    });
  });

  it('gives a text-or-image model a prompt only', () => {
    expect(exampleBody(info('fal/hyper3d-rodin', 'model3d'), rodin)).toEqual({
      model: 'fal/hyper3d-rodin',
      prompt: 'a weathered bronze statue of a fox',
    });
  });

  it('keeps n for an image model, and falls back to a prompt before the schema loads', () => {
    expect(exampleBody(info('mock/visual-image-gen', 'image'), null)).toEqual({
      model: 'mock/visual-image-gen',
      prompt: 'a photo of a cat',
      n: 1,
    });
  });
});

describe('buildCurl', () => {
  it('posts to the family endpoint with the example body', () => {
    const curl = buildCurl(info('fal/trellis', 'model3d'), trellis);
    expect(curl).toContain('$LITEGEN_BASE/v1/models3d/generations');
    expect(curl).toContain('"reference_images":[{"type":"url","value":"https://example.com/input.png","role":"init"}]');
  });

  it('routes each family to its own endpoint', () => {
    expect(generationEndpoint(info('m', 'image'))).toBe('/v1/images/generations');
    expect(generationEndpoint(info('m', 'video'))).toBe('/v1/videos/generations');
    expect(generationEndpoint(info('m', 'model3d'))).toBe('/v1/models3d/generations');
  });
});
