import { describe, expect, it } from 'vitest';
import type { Model3dAsset } from '@litegen/sdk';
import { clipRows, originalOf, sceneDimensions, sceneFacts, sceneNotes, sceneTriangles } from './model3d-assets';

const SCENE = {
  units: 'm', up: '+y', front: '+z', oriented_by: 'litegen',
  triangle_count: 1200, vertex_count: 640, uv_sets: 1, vertex_colors: false,
  materials: [{}, {}], extensions_used: ['KHR_materials_unlit'],
  bounds: { min: [-0.5, 0, -0.25], max: [0.5, 1.75, 0.25] },
  skin: { joint_count: 7, joints: ['hips', 'torso', 'head', 'arm_l', 'arm_r', 'leg_l', 'leg_r'], skeleton: 'humanoid', bind_pose: 't-pose' },
  animations: [
    { name: 'walk', duration_s: 1.2, in_place: true, in_place_by: 'litegen', loop_requested: true, root_travel_m: [0, 0, 0] },
    { name: 'wave', duration_s: 2, in_place: false, root_travel_m: [0.4, 0, 1.1] },
  ],
};

describe('the scene manifest a delivered mesh carries', () => {
  it('reads out its frame, size, materials, bounds and skin', () => {
    expect(sceneFacts(SCENE)).toEqual([
      ['Units / up / front', 'm · +y · +z (oriented by litegen)'],
      ['Triangles / vertices', '1,200 · 640'],
      ['Materials', '2 · 1 UV set · no vertex colours'],
      ['Bounds', '1.00 × 1.75 × 0.50 m'],
      ['Skin', '7 joints · humanoid · t-pose'],
      ['Extensions', 'KHR_materials_unlit'],
    ]);
  });

  it('says nothing for a mesh without one', () => {
    expect(sceneFacts(null)).toEqual([]);
    expect(sceneFacts({ units: 7 })).toEqual([]);
  });

  it('lists each clip with its length, loop and travel', () => {
    expect(clipRows(SCENE)).toEqual([
      { name: 'walk', duration: '1.20 s', loop: 'yes', inPlace: 'yes (litegen)', travel: '0.00 m' },
      { name: 'wave', duration: '2.00 s', loop: '—', inPlace: 'no', travel: '1.17 m' },
    ]);
    expect(clipRows({})).toEqual([]);
  });
});

describe("the vendor's original file", () => {
  it('is the asset of kind original, if delivered', () => {
    const mesh = { kind: 'mesh', url: 'https://x/m.glb', format: 'glb' } as Model3dAsset;
    const original = { kind: 'original', url: 'https://x/o.fbx', format: 'fbx' } as Model3dAsset;
    expect(originalOf([mesh, original])).toBe(original);
    expect(originalOf([mesh])).toBeNull();
  });
});

describe('what the inspector reads from the manifest', () => {
  it('counts the triangles litegen measured', () => {
    expect(sceneTriangles(SCENE)).toBe(1200);
    expect(sceneTriangles({ ...SCENE, triangle_count: 'many' })).toBeNull();
    expect(sceneTriangles(null)).toBeNull();
  });

  it('sizes the mesh from its bounds, in metres, or leaves it to the viewer', () => {
    expect(sceneDimensions(SCENE)).toEqual({ x: 1, y: 1.75, z: 0.5 });
    expect(sceneDimensions({ ...SCENE, bounds: null })).toBeUndefined();
    expect(sceneDimensions({ ...SCENE, bounds: { min: [0, 0], max: [1, 1] } })).toBeUndefined();
    expect(sceneDimensions(undefined)).toBeUndefined();
  });

  it('lists what delivery could not keep, skipping anything that is not text', () => {
    expect(sceneNotes({ ...SCENE, notes: ['a texture could not be decoded', 7, ''] })).toEqual(['a texture could not be decoded']);
    expect(sceneNotes(SCENE)).toEqual([]);
    expect(sceneNotes(null)).toEqual([]);
  });
});
