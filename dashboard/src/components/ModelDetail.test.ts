import { describe, expect, it, vi } from 'vitest';

// ModelDetail imports the shared SDK client, whose module body reads
// localStorage; these tests cover the pure capability-label lookup.
vi.mock('../sdk-client', () => ({ client: {} }));

const { capabilityLabels } = await import('./ModelDetail');

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
