import { describe, expect, it } from 'vitest';
import type { Generation, Model3dAsset } from '@litegen/sdk';
import { isViewerFormat } from '@litegen/3d-viewer/helpers';
import {
  assetsOf,
  validAssets,
  packageDownloads,
  originalOf,
  packageLabel,
  isBrowserImage,
  meshOf,
  previewOf,
  texturesOf,
} from './model3d-assets';

// formatBytes, formatDimensions and copyToClipboard moved to the shared
// viewer package, with their tests (packages/3d-viewer/src/format.test.ts).
// The <model-viewer> workarounds (canPreviewMesh, retrySrc,
// loadEventMatches, classifyViewerError, matchResourceStatus,
// describeViewerError) went with <model-viewer>: the shared viewer
// downloads the file itself and knows its HTTP status
// (packages/3d-viewer/src/viewer/fetch-model.test.ts).

const MESH: Model3dAsset = {
  kind: 'mesh', url: 'https://cdn.example/out/mesh.glb', format: 'glb', size_bytes: 1904, polycount: 12,
};
const PREVIEW: Model3dAsset = {
  kind: 'preview', url: 'https://cdn.example/out/preview.png', format: 'png', width: 512, height: 512,
};
const TEXTURE: Model3dAsset = {
  kind: 'texture', url: 'https://cdn.example/out/albedo.jpg', format: 'jpg', width: 1024, height: 1024,
};

function gen(over: Partial<Generation>): Generation {
  return {
    id: 'litegen-3d-1',
    key_id: null,
    model: 'mock/mesh-3d',
    provider: 'mock',
    media_type: 'model3d',
    status: 'completed',
    progress: 100,
    cost_usd: 0,
    created_at: '2026-09-11T00:00:00Z',
    ...over,
  };
}

describe('assetsOf', () => {
  it('returns metadata.assets when present', () => {
    expect(assetsOf(gen({ metadata: { assets: [MESH, PREVIEW] } }))).toEqual([MESH, PREVIEW]);
  });

  it.each([
    ['missing metadata', undefined],
    ['null metadata', null],
    ['metadata without assets', {}],
    ['null assets', { assets: null }],
    ['non-array assets (object)', { assets: { kind: 'mesh' } }],
    ['non-array assets (string)', { assets: 'mesh.glb' }],
  ])('tolerates %s', (_label, metadata) => {
    expect(assetsOf(gen({ metadata, result_url: null }))).toEqual([]);
  });

  it('drops malformed entries instead of crashing the renderer', () => {
    const metadata = { assets: [null, 7, { kind: 'mesh' }, { url: 'https://x/y.glb' }, MESH] };
    expect(assetsOf(gen({ metadata }))).toEqual([MESH]);
  });

  it('synthesizes the mesh from result_url for rows written before assets[] existed', () => {
    const assets = assetsOf(gen({ metadata: null, result_url: 'https://cdn.example/old/model.GLB?sig=abc' }));
    expect(assets).toEqual([{ kind: 'mesh', url: 'https://cdn.example/old/model.GLB?sig=abc', format: 'glb' }]);
  });

  it('keeps a listed mesh rather than duplicating it from result_url', () => {
    const assets = assetsOf(gen({ metadata: { assets: [MESH, PREVIEW] }, result_url: MESH.url }));
    expect(assets.filter(a => a.kind === 'mesh')).toHaveLength(1);
  });

  it('never invents a mesh for a non-3D row', () => {
    expect(assetsOf(gen({ media_type: 'image', result_url: 'https://cdn.example/a.png' }))).toEqual([]);
  });

  it('drops an entry with no format rather than letting the renderer throw on it', () => {
    const noFormat = { kind: 'preview', url: 'https://cdn.example/p.png' };
    expect(assetsOf(gen({ metadata: { assets: [MESH, noFormat] } }))).toEqual([MESH]);
  });
});

describe('validAssets', () => {
  it('keeps well-formed assets', () => {
    expect(validAssets([MESH, PREVIEW, TEXTURE])).toEqual([MESH, PREVIEW, TEXTURE]);
  });

  it.each([
    ['no format', { kind: 'preview', url: 'https://x/p.png' }],
    ['numeric format', { kind: 'preview', url: 'https://x/p.png', format: 7 }],
    ['unknown kind', { kind: 'skeleton', url: 'https://x/s.bin', format: 'bin' }],
    ['numeric url', { kind: 'mesh', url: 42, format: 'glb' }],
    ['empty url', { kind: 'mesh', url: '', format: 'glb' }],
    ['null', null],
    ['a string', 'https://x/m.glb'],
  ])('drops an entry with %s', (_label, entry) => {
    expect(validAssets([entry, MESH])).toEqual([MESH]);
  });

  it.each([undefined, null, {}, 'mesh.glb', 3])('is empty for non-array input %s', raw => {
    expect(validAssets(raw)).toEqual([]);
  });
});

describe('meshOf', () => {
  it('finds the mesh asset', () => {
    expect(meshOf([PREVIEW, MESH, TEXTURE])).toBe(MESH);
  });

  it('prefers the glb, as the backend does for result_url, over an earlier gltf', () => {
    // litegen converts to gltf too, so both can come back; the viewer could
    // render either, but the inspector must show the same mesh the row links.
    const gltf = { kind: 'mesh' as const, url: 'https://cdn.example/r/model.gltf', format: 'gltf' };
    const obj = { kind: 'mesh' as const, url: 'https://cdn.example/r/model.obj', format: 'obj' };
    const glb = { kind: 'mesh' as const, url: 'https://cdn.example/r/model.glb', format: 'GLB' };
    expect(meshOf([obj, gltf, glb])).toBe(glb);
    // No glb: the next renderable one, not the first listed.
    expect(meshOf([obj, gltf])).toBe(gltf);
  });

  it('falls back to result_url when no mesh is listed', () => {
    expect(meshOf([PREVIEW], 'https://cdn.example/r/mesh.obj')).toEqual({
      kind: 'mesh', url: 'https://cdn.example/r/mesh.obj', format: 'obj',
    });
  });

  it('prefers the listed mesh over result_url', () => {
    expect(meshOf([MESH], 'https://cdn.example/other.glb')).toBe(MESH);
  });

  it('leaves format empty when the fallback URL has no extension', () => {
    expect(meshOf([], 'https://cdn.example/blob/123')?.format).toBe('');
  });

  it('is null with neither a mesh nor a fallback', () => {
    expect(meshOf([PREVIEW])).toBeNull();
    expect(meshOf([PREVIEW], null)).toBeNull();
    expect(meshOf([], '')).toBeNull();
  });
});

describe('previewOf', () => {
  it('finds the preview asset', () => {
    expect(previewOf([MESH, TEXTURE, PREVIEW])).toBe(PREVIEW);
  });

  it('is null when the provider returned none', () => {
    expect(previewOf([MESH, TEXTURE])).toBeNull();
  });
});

describe('texturesOf', () => {
  it('excludes the mesh and the preview', () => {
    expect(texturesOf([MESH, PREVIEW, TEXTURE])).toEqual([TEXTURE]);
  });

  it('keeps every texture, in order', () => {
    const normal = { ...TEXTURE, url: 'https://cdn.example/out/normal.png', format: 'png' };
    expect(texturesOf([TEXTURE, MESH, normal])).toEqual([TEXTURE, normal]);
  });
});

describe('isBrowserImage', () => {
  it.each(['png', 'jpg', 'JPEG', 'webp', 'gif'])('%s renders in an <img>', f => {
    expect(isBrowserImage(f)).toBe(true);
  });

  it.each(['glb', 'ktx2', 'exr', ''])('%s does not', f => {
    expect(isBrowserImage(f)).toBe(false);
  });
});

describe('meshOf with several containers', () => {
  // A generation can now carry one mesh per requested container, because
  // litegen derives obj/stl/ply locally from a vendor's glb.
  const mesh = (format: string): Model3dAsset =>
    ({ kind: 'mesh', url: `https://x/model.${format}`, format }) as Model3dAsset;

  it('prefers the glb whatever order the assets arrive in', () => {
    // The same mesh the backend links as result_url, and the richest.
    expect(meshOf([mesh('stl'), mesh('obj'), mesh('glb')])?.format).toBe('glb');
    expect(meshOf([mesh('glb'), mesh('obj')])?.format).toBe('glb');
  });

  it('accepts gltf as previewable too', () => {
    expect(meshOf([mesh('stl'), mesh('gltf')])?.format).toBe('gltf');
  });

  it('takes the first mesh the viewer draws when there is no glTF', () => {
    expect(meshOf([mesh('fbx'), mesh('stl')])?.format).toBe('fbx');
    // A container the viewer has no loader for is passed over.
    expect(meshOf([mesh('xyz'), mesh('stl')])?.format).toBe('stl');
  });

  it('falls back to the first mesh when the viewer draws none of them', () => {
    // Still better than null: the asset table and the download links work even
    // when the viewer cannot render it.
    expect(meshOf([mesh('xyz'), mesh('abc')])?.format).toBe('xyz');
  });

  it('still backfills from result_url when there are no listed meshes', () => {
    expect(meshOf([], 'https://x/model.glb')?.format).toBe('glb');
    expect(meshOf([])).toBeNull();
  });
});

describe('packageDownloads', () => {
  const pkg = (format: string): Model3dAsset =>
    ({ kind: 'package', format, url: `https://cdn.test/g/package_${format}.zip` }) as Model3dAsset;
  const MESH_GLB = { kind: 'mesh', format: 'glb', url: 'https://cdn.test/g/mesh.glb' } as Model3dAsset;

  it('keeps package assets through validation instead of filtering them out', () => {
    expect(validAssets([MESH_GLB, pkg('obj')])).toEqual([MESH_GLB, pkg('obj')]);
  });

  it('lists only the packages, in the order the API sent them', () => {
    expect(packageDownloads([MESH_GLB, pkg('usda'), pkg('obj'), pkg('dae')])).toEqual([pkg('usda'), pkg('obj'), pkg('dae')]);
    expect(packageDownloads([MESH_GLB])).toEqual([]);
  });

  it('labels a package by the container inside it', () => {
    expect(packageLabel(pkg('obj'))).toBe('Download OBJ package (.zip)');
  });

  it('never offers a package to the mesh viewer', () => {
    expect(isViewerFormat('zip')).toBe(false);
  });
});

describe('original assets', () => {
  it("keeps the vendor's original file through validation so originalOf finds it", () => {
    const mesh = { kind: 'mesh', format: 'glb', url: 'https://cdn.test/g/mesh.glb' } as Model3dAsset;
    const original = { kind: 'original', format: 'fbx', url: 'https://cdn.test/g/original.fbx' } as Model3dAsset;
    const assets = validAssets([mesh, original]);
    expect(assets).toEqual([mesh, original]);
    expect(originalOf(assets)).toEqual(original);
  });
});
