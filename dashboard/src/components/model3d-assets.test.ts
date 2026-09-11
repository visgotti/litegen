import { describe, expect, it } from 'vitest';
import type { Generation, Model3dAsset } from '@litegen/sdk';
import {
  assetsOf,
  canPreviewMesh,
  copyToClipboard,
  loadEventMatches,
  retrySrc,
  validAssets,
  classifyViewerError,
  describeViewerError,
  formatBytes,
  formatDimensions,
  isBrowserImage,
  meshOf,
  previewOf,
  texturesOf,
} from './model3d-assets';

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

describe('canPreviewMesh', () => {
  it.each(['glb', 'GLB', 'gltf', 'GlTf'])('%s renders in <model-viewer>', f => {
    expect(canPreviewMesh(f)).toBe(true);
  });

  it.each(['obj', 'fbx', 'usdz', 'OBJ'])('%s does not (model-viewer is glTF-only)', f => {
    expect(canPreviewMesh(f)).toBe(false);
  });

  it('attempts an unknown (empty) format and lets the viewer decide', () => {
    expect(canPreviewMesh('')).toBe(true);
  });

  it.each([7, null, { ext: 'glb' }])('rejects a non-string format (%s) instead of throwing', f => {
    expect(() => canPreviewMesh(f)).not.toThrow();
    expect(canPreviewMesh(f)).toBe(false);
  });
});

describe('retrySrc', () => {
  const SRC = 'https://cdn.example/out/mesh.glb?X-Amz-Signature=abc';

  it('is the raw src before any retry', () => {
    expect(retrySrc(SRC, 0)).toBe(SRC);
  });

  it('adds a slash-free fragment per attempt, leaving path and query untouched', () => {
    expect(retrySrc(SRC, 1)).toBe(`${SRC}#retry-1`);
    expect(retrySrc(SRC, 2)).toBe(`${SRC}#retry-2`);
    const busted = new URL(retrySrc(SRC, 3));
    expect(busted.search).toBe('?X-Amz-Signature=abc');
    expect(busted.hash).not.toContain('/');
  });

  it('still produces a distinct string when the src already has a fragment', () => {
    expect(retrySrc('https://x/m.glb#v2', 1)).not.toBe('https://x/m.glb#v2');
  });
});

describe('loadEventMatches', () => {
  const SRC = 'https://cdn.example/new.glb';

  it('accepts the load for the current src', () => {
    expect(loadEventMatches({ url: SRC }, SRC)).toBe(true);
  });

  it('rejects a stale load for a src that has since been replaced', () => {
    expect(loadEventMatches({ url: 'https://cdn.example/old.glb' }, SRC)).toBe(false);
  });

  it('accepts a detail without a url rather than never settling', () => {
    expect(loadEventMatches(undefined, SRC)).toBe(true);
    expect(loadEventMatches({}, SRC)).toBe(true);
  });
});

describe('meshOf', () => {
  it('finds the mesh asset', () => {
    expect(meshOf([PREVIEW, MESH, TEXTURE])).toBe(MESH);
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

describe('formatBytes', () => {
  it.each([
    [0, '0 B'],
    [1, '1 B'],
    [1023, '1023 B'],
    [1024, '1.0 KB'],
    [1536, '1.5 KB'],
    // Rounds up into the next unit rather than printing "1024.0 KB".
    [1024 * 1024 - 1, '1.0 MB'],
    [1.5 * 1024 * 1024, '1.5 MB'],
    [3 * 1024 ** 3, '3.0 GB'],
  ])('%d -> %s', (n, want) => {
    expect(formatBytes(n)).toBe(want);
  });

  it.each([null, undefined, -1, Number.NaN, Number.POSITIVE_INFINITY])('renders %s as a dash', n => {
    expect(formatBytes(n)).toBe('—');
  });
});

describe('formatDimensions', () => {
  it('formats a bounding box in metres', () => {
    expect(formatDimensions({ x: 1, y: 1, z: 1 })).toBe('1.00 × 1.00 × 1.00 m');
    expect(formatDimensions({ x: 2, y: 0.5, z: 1.25 })).toBe('2.00 × 0.50 × 1.25 m');
  });

  it('does not collapse a millimetre-scale model to zero', () => {
    expect(formatDimensions({ x: 0.0005, y: 0.001, z: 0 })).toBe('5.0e-4 × 1.0e-3 × 0.00 m');
  });

  it('renders an unknown box as a dash', () => {
    expect(formatDimensions(null)).toBe('—');
    expect(formatDimensions({ x: Number.NaN, y: 1, z: 1 })).toBe('—');
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

describe('describeViewerError', () => {
  it('explains a load failure', () => {
    expect(describeViewerError({ type: 'loadfailure' })).toMatch(/could not be loaded/i);
  });

  it('explains a lost WebGL context', () => {
    expect(describeViewerError({ type: 'webglcontextlost' })).toMatch(/webgl/i);
  });

  it('has a generic message for anything else', () => {
    expect(describeViewerError(undefined)).toMatch(/could not render/i);
    expect(describeViewerError({ type: 'something-new' })).toMatch(/could not render/i);
  });

  it('explains a parse failure, overriding the load-failure text', () => {
    expect(describeViewerError({ type: 'loadfailure' }, 'parse')).toMatch(/isn't a valid gltf/i);
  });
});

describe('classifyViewerError', () => {
  // Both a download failure and a parse failure dispatch the exact same
  // `detail` in the installed model-viewer 4.3.1 — a generic `loadfailure`
  // with no way to tell them apart (see the doc comment on the function and
  // the Task 17B report for the real-browser evidence). responseStatus, read
  // by the caller from PerformanceResourceTiming for the same request, is
  // the only signal that discriminates.
  it('is a download failure when no response was ever observed (404, network error, or no timing entry)', () => {
    expect(classifyViewerError({ type: 'loadfailure' }, null)).toBe('load');
  });

  it('is a download failure for a non-2xx/3xx status', () => {
    expect(classifyViewerError({ type: 'loadfailure' }, 404)).toBe('load');
    expect(classifyViewerError({ type: 'loadfailure' }, 500)).toBe('load');
  });

  it('is a download failure for an opaque cross-origin status (0)', () => {
    // The common case for object storage that does not send
    // Timing-Allow-Origin: the resource loaded fine, but we cannot tell, so
    // this must default to the safe choice (Retry stays available).
    expect(classifyViewerError({ type: 'loadfailure' }, 0)).toBe('load');
  });

  it('is a parse failure when the bytes were received (2xx/3xx) but the load still failed', () => {
    expect(classifyViewerError({ type: 'loadfailure' }, 200)).toBe('parse');
    expect(classifyViewerError({ type: 'loadfailure' }, 304)).toBe('parse');
  });

  it('is never a parse failure for an unknown or unrelated detail.type, even with a successful status', () => {
    // e.g. webglcontextlost, which can fire long after a successful load and
    // would otherwise pick up that load's own (unrelated) 200 timing entry.
    expect(classifyViewerError({ type: 'webglcontextlost' }, 200)).toBe('load');
    expect(classifyViewerError({ type: 'something-new' }, 200)).toBe('load');
    expect(classifyViewerError(undefined, 200)).toBe('load');
    expect(classifyViewerError(null, 200)).toBe('load');
  });
});

describe('copyToClipboard', () => {
  it('reports success when the write resolves', async () => {
    const writes: string[] = [];
    const clipboard = { writeText: async (t: string) => { writes.push(t); } };
    await expect(copyToClipboard('https://x/m.glb', clipboard)).resolves.toBe(true);
    expect(writes).toEqual(['https://x/m.glb']);
  });

  it('reports failure without throwing when the clipboard API is missing', async () => {
    await expect(copyToClipboard('u', undefined)).resolves.toBe(false);
  });

  it('reports failure without throwing when the write is denied', async () => {
    const clipboard = { writeText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) };
    await expect(copyToClipboard('u', clipboard)).resolves.toBe(false);
  });
});
