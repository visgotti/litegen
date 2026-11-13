import { describe, expect, it } from 'vitest';
import type { ModelInfo } from '@litegen/3d-converter';
import { clipRows, modelFacts } from './format-helpers';

// formatBytes, formatDimensions and drawnNote are tested where they live now,
// in @litegen/3d-viewer (packages/3d-viewer/src/format.test.ts).
const skinned: ModelInfo = {
  triangles: 1200,
  vertices: 800,
  nodes: 5,
  meshes: 1,
  textures: 2,
  uvSets: 1,
  vertexColors: false,
  bounds: { min: [-0.5, 0, -0.25], max: [0.5, 1.8, 0.25] },
  materials: [{ name: 'skin', maps: ['base_color', 'normal'] }],
  skin: {
    joints: 24,
    root: 'Hips',
    bindPose: 't',
    humanoid: { naming: 'mixamo', bones: [{ bone: 'hips', joint: 'mixamorig:Hips' }, { bone: 'head', joint: 'mixamorig:Head' }] },
  },
  animations: [{ name: 'walk', duration: 1.25, rootTravel: [0, 0, 3], inPlace: false }],
};

describe('modelFacts', () => {
  it('lists what the reader measured', () => {
    expect(modelFacts(skinned)).toEqual([
      ['Triangles / vertices', '1,200 · 800'],
      ['Meshes / nodes', '1 · 5'],
      ['Materials', '1 · 1 UV set · no vertex colours'],
      ['Textures', '2'],
      ['Bounds', '1.00 × 1.80 × 0.50 m'],
      ['Skin', '24 joints · Hips · T-pose · mixamo humanoid, 2 bones'],
    ]);
  });
  it('leaves out what the model does not have', () => {
    const plain: ModelInfo = { ...skinned, textures: 0, uvSets: 2, vertexColors: true, materials: [], animations: [] };
    delete (plain as { skin?: unknown }).skin;
    delete (plain as { bounds?: unknown }).bounds;
    expect(modelFacts(plain)).toEqual([
      ['Triangles / vertices', '1,200 · 800'],
      ['Meshes / nodes', '1 · 5'],
      ['Materials', '0 · 2 UV sets · vertex colours'],
    ]);
  });
});

describe('clipRows', () => {
  it('names each clip with its length, place and travel', () => {
    expect(clipRows(skinned)).toEqual([{ name: 'walk', duration: '1.25 s', inPlace: 'no', travel: '3.00 m' }]);
    expect(clipRows({ ...skinned, animations: [{ duration: 2, rootTravel: [0, 0, 0], inPlace: true }] })).toEqual([
      { name: '—', duration: '2.00 s', inPlace: 'yes', travel: '0.00 m' },
    ]);
  });
});

